# Unified Event Pipeline Design

## Problem

The nexus-store crate has three interleaved concerns on the read and write paths:

- **Schema migration** (upcasting) — transforming old event schemas to current
- **Codec** (encode/decode) — converting between domain events and bytes
- **Validation** — ensuring schema versions advance, event types are non-empty, iteration limits

These are currently handled by separate traits (`EventUpcaster`, `Codec`, `BorrowingCodec`) with validation inline in a monolithic loop. The write path (`save_with_encoder`) is broken after the kernel refactor that removed event buffering from `AggregateRoot`.

## Design

### Core principle

The read and write paths are pipelines of Cow-based transformations:

```
READ:  cursor buffer → [upcast]* → [decode] → domain event
WRITE: domain event → [encode] → [schema version lookup] → raw bytes
```

Each step either borrows (Cow::Borrowed, zero-copy) or allocates (Cow::Owned, data changed). The common case — event already at current schema version — is zero-allocation.

### Two profiles

| | Server (`std`) | IoT (`no_std + alloc`) |
|---|---|---|
| Pipeline | GAT lending + Cow threading | Same |
| Tower integration | `impl Service<...>` on EventStore | Not available |
| Allocation | Acceptable | Minimized via Cow |
| Feature | `tower` feature flag | Default (core) |

Tower impls are thin adapters on the same types (not wrappers). They accept the owned-response cost.

### EventMorsel — the unit of data flowing through the pipeline

Inspired by Polars' `Morsel` pattern: data + metadata wrapped in a single
type that flows through each pipeline step. Instead of passing loose
`(event_type, schema_version, payload)` tuples, the pipeline passes morsels.

```rust
/// A single event as it flows through the transform pipeline.
///
/// Borrows from the cursor buffer when no transform has fired (zero-copy).
/// Becomes owned after the first transform allocates.
pub struct EventMorsel<'a> {
    pub event_type: Cow<'a, str>,
    pub schema_version: Version,
    pub payload: Cow<'a, [u8]>,
}
```

Each pipeline step takes `EventMorsel<'a>` and returns `EventMorsel<'a>`:
- Transform didn't fire → morsel passes through unchanged (still borrowed)
- Transform fired → morsel fields become `Cow::Owned` where data changed

The morsel is constructed from `PersistedEnvelope` on the read path
(borrowing from cursor buffer) and consumed by the codec at the end.
On the write path, the morsel is constructed from the encoded event
and consumed by `PendingEnvelope` building.

`TransformChain::apply` takes and returns a morsel:

```rust
pub trait TransformChain: Send + Sync {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> EventMorsel<'a>;
    fn can_transform(&self, event_type: &str, schema_version: Version) -> bool;
}
```

The pipeline becomes:

```rust
// Read path — zero-alloc when no transform fires
let morsel = EventMorsel::from_envelope(&env);  // all Cow::Borrowed
let morsel = chain.apply(morsel);                // borrowed or owned
let morsel = validate(morsel)?;                  // standalone check
let event = codec.decode(&morsel.event_type, &morsel.payload)?;
```

### Traits

#### `SchemaTransform` — replaces `EventUpcaster`

```rust
pub trait SchemaTransform: Send + Sync {
    fn event_type(&self) -> &str;
    fn from_version(&self) -> Version;
    fn to_version(&self) -> Version;
    fn transform<'a>(&self, payload: &'a [u8]) -> Cow<'a, [u8]>;

    /// Override if the transform also renames the event type.
    fn new_event_type(&self) -> Option<&str> { None }
}
```

Declarative: says WHAT it does (event_type, from_version, to_version). The pipeline handles matching and chaining. No `try_upcast`, no `Option`, no matching logic on the trait.

#### `SchemaRegistry` — pluggable version lookup

```rust
pub trait SchemaRegistry: Send + Sync {
    fn current_version(&self, event_type: &str) -> Option<Version>;
}
```

Default impl `ChainDerivedRegistry<C>` walks the HList chain to derive the current version. Future impls: remote registries (Confluent, Apicurio), local config.

#### `TransformChain` — replaces `UpcasterChain`

```rust
pub trait TransformChain: Send + Sync {
    /// Apply the first matching transform to the morsel.
    /// Returns the morsel unchanged if no transform matches.
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> EventMorsel<'a>;

    /// Check if any transform handles this event type + version.
    /// Used by SchemaRegistry to probe the current version.
    fn can_transform(&self, event_type: &str, schema_version: Version) -> bool;
}
```

HList impls:
- `()` base case: returns morsel unchanged, `can_transform` returns false
- `Chain<H, T>` recursive case: if head matches `(morsel.event_type, morsel.schema_version)`, apply head's transform and update morsel; otherwise delegate to tail

#### `Repository` — updated save signature

```rust
pub trait Repository<A: Aggregate>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load(&self, id: A::Id)
        -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

### Validation — standalone middleware functions

Each validation is a standalone function applied by the pipeline. Not on any trait:

- `check_version_advanced(prev, next)` — schema version must strictly increase
- `check_event_type_nonempty(event_type)` — event type must not be empty after transform
- `check_iteration_limit(iterations, limit)` — bound total transform steps

### Pipeline execution

#### Read path (in EventStore::load)

```
for each envelope from EventStream (GAT lending cursor):
  1. morsel = EventMorsel::from_envelope(&env)     // all Cow::Borrowed
  2. loop:
       prev_version = morsel.schema_version
       morsel = chain.apply(morsel)                 // finds matching transform or passes through
       if morsel.schema_version == prev_version:
         break                                      // no transform fired, done
       validate_advance(prev_version, morsel)?      // version must increase, type non-empty
       check_iteration_limit(iterations)?
  3. event = codec.decode(&morsel.event_type, &morsel.payload)?
  4. root.replay(env.version(), &event)?
```

Zero allocation when morsel passes through unchanged (common case).

#### Write path (in EventStore::save)

```
1. if events.is_empty() → return Ok(())
2. expected_version = aggregate.version()
3. for each event in events:
     a. version = compute next version (checked arithmetic)
     b. payload = codec.encode(event)
     c. schema_version = registry.current_version(event.name())
     d. build PendingEnvelope(version, event.name(), schema_version, payload)
4. RawEventStore::append(id, expected_version, &envelopes)
5. on success:
     a. aggregate.advance_version(last_version)
     b. for each event: aggregate.apply_event(&event)
6. on conflict: map to StoreError::Conflict
```

### Kernel change (unfreeze)

Add one method to `AggregateRoot<A>`:

```rust
pub fn apply_event(&mut self, event: &EventOf<A>) {
    let new_state = self.state.clone().apply(event);
    self.state = new_state;
}
```

Single-event version of `apply_events`. Clone-based for panic safety. This is the only kernel change.

### Tower feature gate

```rust
#[cfg(feature = "tower")]
impl<A, S, C, U> Service<LoadRequest<A::Id>> for EventStore<S, C, U> { ... }

#[cfg(feature = "tower")]
impl<A, S, C, U> Service<SaveRequest<'_, A>> for EventStore<S, C, U> { ... }
```

Thin wrappers over the same internal logic. Owned types (no Cow, no GAT). Users get tower middleware ecosystem (logging, metrics, timeout). Dependency: `tower-service` only (no_std, zero deps).

### Dependencies

| Crate | Feature | Purpose |
|-------|---------|---------|
| `tower-service` | `tower` feature | `Service` trait only |

No other new dependencies.

### What gets removed

- `EventUpcaster` trait (replaced by `SchemaTransform`)
- `UpcasterChain` trait (replaced by `TransformChain`)
- `apply_upcasters` function (was dynamic dispatch, removed)
- `validated_upcast_loop` (replaced by pipeline with standalone validation)
- `save_with_encoder` (replaced by pipeline-based save)

### Migration

Existing `EventUpcaster` impls change from:

```rust
impl EventUpcaster for V1ToV2 {
    fn try_upcast<'a>(&self, event_type: &'a str, schema_version: Version, payload: &'a [u8])
        -> Option<(Cow<'a, str>, Version, Cow<'a, [u8]>)> {
        if event_type != "OrderCreated" || schema_version.as_u64() != 1 { return None; }
        Some((Cow::Borrowed(event_type), Version::new(2).unwrap(), Cow::Owned(transform(payload))))
    }
}
```

To:

```rust
impl SchemaTransform for V1ToV2 {
    fn event_type(&self) -> &str { "OrderCreated" }
    fn from_version(&self) -> Version { Version::INITIAL }
    fn to_version(&self) -> Version { Version::new(2).unwrap() }
    fn transform<'a>(&self, payload: &'a [u8]) -> Cow<'a, [u8]> { Cow::Owned(transform(payload)) }
}
```

Cleaner: no matching logic, no Option, pure declaration + transformation.
