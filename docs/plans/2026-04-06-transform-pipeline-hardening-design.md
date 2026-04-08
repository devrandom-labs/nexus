# Transform Pipeline Hardening — Design

**Date:** 2026-04-06
**Status:** Approved
**Scope:** `nexus-store`, `nexus-macros`

## Problem

1. `transform()` is infallible — corrupt data panics.
2. Contiguous versions not enforced — v1→v3 causes silent mismatch.
3. No duplicate detection — shadowed transforms.
4. No aggregate scoping.

## Design

The proc macro is the enforcer. It validates at compile time and generates an enum + `Upcaster` impl.

### What the user writes

```rust
#[nexus::transforms(aggregate = Order)]
impl OrderTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",currency:USD");
        Ok(data)
    }

    #[transform(event = "OrderCreated", from = 2, to = 3)]
    fn v2_to_v3(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(rename_field(payload, "product", "product_id"))
    }

    #[transform(event = "OrderCancelled", from = 1, to = 2, rename = "OrderVoided")]
    fn cancelled_to_voided(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }
}

let es = EventStore::with_upcaster(store, codec, OrderTransforms);
```

Transforms are plain functions: `&[u8] -> Result<Vec<u8>, Error>`.
Event type, versions, rename — all from attributes.

### Macro validates at compile time

- No duplicate `(event, from)` pairs → compile error
- `to == from + 1` → compile error
- `from >= 1` → compile error

### What the macro generates

```rust
struct OrderTransforms;

enum OrderUpcaster {
    CreatedV1ToV2,
    CreatedV2ToV3,
    CancelledToVoided,
}

impl OrderUpcaster {
    fn execute(&self, payload: &[u8]) -> Result<Vec<u8>, UpcastError> {
        match self {
            Self::CreatedV1ToV2 => OrderTransforms::v1_to_v2(payload)
                .map_err(|e| UpcastError::TransformFailed {
                    event_type: "OrderCreated".into(),
                    schema_version: Version::INITIAL,
                    source: Box::new(e),
                }),
            Self::CreatedV2ToV3 => OrderTransforms::v2_to_v3(payload)
                .map_err(|e| UpcastError::TransformFailed {
                    event_type: "OrderCreated".into(),
                    schema_version: Version::new(2).unwrap(),
                    source: Box::new(e),
                }),
            Self::CancelledToVoided => OrderTransforms::cancelled_to_voided(payload)
                .map_err(|e| UpcastError::TransformFailed {
                    event_type: "OrderCancelled".into(),
                    schema_version: Version::INITIAL,
                    source: Box::new(e),
                }),
        }
    }
}

impl Upcaster for OrderTransforms {
    fn apply<'a>(&self, mut morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        loop {
            let (event_type, version, payload) = match (morsel.event_type(), morsel.schema_version()) {
                ("OrderCreated", v) if v == v(1) => {
                    ("OrderCreated", v(2), OrderUpcaster::CreatedV1ToV2.execute(morsel.payload())?)
                }
                ("OrderCreated", v) if v == v(2) => {
                    ("OrderCreated", v(3), OrderUpcaster::CreatedV2ToV3.execute(morsel.payload())?)
                }
                ("OrderCancelled", v) if v == v(1) => {
                    ("OrderVoided", v(2), OrderUpcaster::CancelledToVoided.execute(morsel.payload())?)
                }
                _ => break,
            };
            morsel = EventMorsel::new(event_type, version, payload);
        }
        Ok(morsel)
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "OrderCreated" => Some(v(3)),
            "OrderCancelled" => Some(v(2)),
            _ => None,
        }
    }
}
```

### Upcaster trait

```rust
pub trait Upcaster: Send + Sync {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError>;
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

impl Upcaster for () {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        Ok(morsel)
    }
    fn current_version(&self, _: &str) -> Option<Version> { None }
}
```

### EventMorsel

Single representation of event data. Built from store envelopes on read, transformed by upcasters, fed to codecs.

```rust
pub struct EventMorsel<'a> {
    pub(crate) event_type: Cow<'a, str>,
    pub(crate) schema_version: Version,
    pub(crate) payload: Cow<'a, [u8]>,
}

impl<'a> EventMorsel<'a> {
    /// From cursor buffer — zero copy.
    pub fn borrowed(event_type: &'a str, schema_version: Version, payload: &'a [u8]) -> Self;

    /// From transform output — owned payload.
    pub fn new(event_type: &str, schema_version: Version, payload: Vec<u8>) -> Self;

    pub fn event_type(&self) -> &str;
    pub fn schema_version(&self) -> Version;
    pub fn payload(&self) -> &[u8];
    pub fn is_borrowed(&self) -> bool;
}
```

### EventStore

```rust
pub struct EventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcaster: U,
}

impl<S, C> EventStore<S, C, ()> {
    pub const fn new(store: S, codec: C) -> Self;
}

impl<S, C, U: Upcaster> EventStore<S, C, U> {
    pub fn with_upcaster(store: S, codec: C, upcaster: U) -> Self;
}
```

- **Read:** envelope → `EventMorsel::borrowed(...)` → `upcaster.apply(morsel)` → `codec.decode(morsel.payload())`
- **Write:** `upcaster.current_version(event_name)` → stamp schema version on envelope

### UpcastError

```rust
pub enum UpcastError {
    // ...existing variants...

    #[error("Transform failed for '{event_type}' at schema version {schema_version}: {source}")]
    TransformFailed {
        event_type: String,
        schema_version: Version,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
```

### Allocation budget

| Path | Heap allocations |
|------|-----------------|
| Read — no transform fires | 0 |
| Read — transform fires | 1 per step (cold path) |
| Write | 0 |
| Error | 1 `Box` + 1 `String` (cold path) |

## Deleted

- `TransformedEvent`
- `TransformChain` trait
- `transform_chain.rs`, `Chain<H, T>`
- `schema_registry.rs`, `SchemaRegistry`, `ChainDerivedRegistry`
- `pipeline.rs` — logic moves into macro-generated `Upcaster::apply`

## Migration

- `EventStore::new(store, codec).with_transform(T)` → `EventStore::with_upcaster(store, codec, OrderTransforms)`
- Transforms become `&[u8] -> Result<Vec<u8>, Error>` functions via macro
- No breaking change for `Repository` users

## Test Plan

1. Transform returns `Err` → `UpcastError::TransformFailed`
2. Macro rejects duplicate `(event, from)` → compile error
3. Macro rejects `to != from + 1` → compile error
4. Multi-step: v1 event → upcasted to v3
5. No-op: current-version event passes through, `morsel.is_borrowed()`
6. `current_version` correct per event type
7. `()` upcaster is passthrough
8. End-to-end: macro-generated upcaster in `EventStore` load/save
