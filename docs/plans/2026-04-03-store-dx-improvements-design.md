# Store DX Improvements Design

**Date:** 2026-04-03
**Status:** Approved
**Branch:** feat/nexus-store

## Problem

The nexus-store examples reveal five DX pain points:

1. No repository layer — every user writes ~50 lines of encode/decode/persist glue
2. No in-memory store — every test suite builds its own ~100-line implementation
3. `EventStream` consumption requires a 12-line manual loop with copy-out
4. `schema_version` missing from envelope types — upcasters can't get it from the envelope
5. `apply_event` / `load_from_events` naming and streaming rehydration gap

## Design Decisions

### 1. `replay(version, event)` on `AggregateRoot`

Single-event streaming rehydration step. Validates version is strictly sequential,
applies to state, advances persisted version. Does NOT record as uncommitted.

```rust
impl<A: Aggregate> AggregateRoot<A> {
    pub fn replay(
        &mut self,
        version: Version,
        event: EventOf<A>,
    ) -> Result<(), KernelError> {
        let expected = self.version.next();
        if version != expected {
            return Err(KernelError::VersionMismatch {
                stream_id: ErrorId::from_display(&self.id),
                expected,
                actual: version,
            });
        }
        if version.as_u64() > A::MAX_REHYDRATION_EVENTS as u64 {
            return Err(KernelError::RehydrationLimitExceeded {
                stream_id: ErrorId::from_display(&self.id),
                max: A::MAX_REHYDRATION_EVENTS,
            });
        }
        self.state.apply(&event);
        self.version = version;
        Ok(())
    }
}
```

`load_from_events` is **removed**. The only way to rehydrate is streaming via `replay()`.
This enables zero-intermediate-allocation rehydration from a lending cursor.

### 2. Rename `apply_event` to `apply`

- `apply(event)` — new event, records as uncommitted, for command handling
- `replay(version, event)` — historical event, for rehydration

`apply_events` becomes `apply_all` or is removed (callers can loop).

### 3. `schema_version` on `PersistedEnvelope` (read path only)

Added to `PersistedEnvelope` only. `PendingEnvelope` stays unchanged — writes are
always current schema.

```rust
pub struct PersistedEnvelope<'a, M = ()> {
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    schema_version: u32,    // NEW — panics on 0 at construction
    payload: &'a [u8],
    metadata: M,
}
```

Construction panics if `schema_version == 0` (programming error, not recoverable).
No upper bound cap beyond `u32` — reject 0 only.

### 4. `Repository<A>` trait

Bridge between kernel aggregates and raw store. Lives in `nexus-store`.

```rust
pub trait Repository<A: Aggregate>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load(
        &self,
        stream_id: &str,
        id: A::Id,
    ) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

Key properties:
- Trait (not concrete struct) — for future axum extractor-style framework integration
- `stream_id` + `id` separate — repository doesn't decide stream naming
- `save` takes `&mut AggregateRoot` to call `take_uncommitted_events()` internally
- Internally wires: codec encode/decode, version tracking, streaming rehydration
- No intermediate `Vec` allocation — streams envelope-by-envelope through `replay()`

Internal `load()` implementation pattern (for implementors):
```rust
let mut stream = store.read_stream(stream_id, Version::INITIAL).await?;
let mut root = AggregateRoot::<A>::new(id);
while let Some(result) = stream.next().await {
    let env = result?;
    let event = codec.decode(env.event_type(), env.payload())?;
    root.replay(env.version(), event)?;
    // env dropped — lending iterator satisfied
}
Ok(root)
```

### 5. `InMemoryStore` behind `testing` feature

Lives in `nexus-store/src/testing.rs`, gated behind `#[cfg(feature = "testing")]`.

```rust
pub struct InMemoryStore {
    streams: tokio::sync::Mutex<HashMap<String, Vec<StoredRow>>>,
}
```

Provides:
- `RawEventStore` implementation with optimistic concurrency
- Sequential version validation on append
- `schema_version` stored per row (aligns with Section 3)
- `EventStream` implementation via lending cursor

Examples and test suites switch to importing this.

## What We Deliberately Did NOT Add

- **`From` conversions** between `VersionedEvent` and envelope types — the repository
  handles wiring internally, no intermediate types needed. Users going manual have
  the building blocks.
- **Default `JsonCodec`** — will be an extension crate behind a feature flag, not
  part of this change.
- **`PendingEnvelope` builder convenience** — same, extension crate territory.
- **`EventStream` combinators** (collect, map, etc.) — the repository absorbs the
  consumption complexity. Direct stream consumers can use the manual loop.

## Migration Impact

- `apply_event()` → `apply()` — rename across all call sites
- `apply_events()` → decide: `apply_all()` or remove
- `load_from_events()` → removed, use `replay()` loop or repository
- `PersistedEnvelope::new()` — gains `schema_version` parameter
- Test suites — switch to `InMemoryStore` from `testing` feature
- Examples — rewrite to use repository trait or simplified patterns

## Crate Boundaries

```
nexus (kernel)          nexus-store
├── AggregateRoot       ├── Repository<A> trait      (NEW)
│   ├── apply()         ├── RawEventStore trait
│   ├── replay()  (NEW) ├── EventStream trait
│   └── new()           ├── Codec<E> trait
├── Version             ├── PendingEnvelope
├── VersionedEvent      ├── PersistedEnvelope  (+ schema_version)
└── DomainEvent         ├── EventUpcaster
                        └── testing::InMemoryStore   (NEW)
```
