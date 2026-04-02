# nexus-store Envelope & Trait Design

**Date:** 2026-04-02
**Status:** Approved
**Scope:** Detailed component design for nexus-store crate

## Overview

Two envelope types, four traits, one facade. Zero-allocation reads. Pluggable codec and upcasters.

## Envelope Types

### PendingEnvelope<M> (write path)

Fully owned. Constructed by the `EventStore` facade from typed events + codec output. Sent to adapter for persistence.

```rust
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,  // from DomainEvent::name()
    payload: Vec<u8>,          // from Codec::encode()
    metadata: M,               // user-defined
}
```

All fields private. `pub(crate)` constructor. Read-only accessors.

### PersistedEnvelope<'a, M> (read path)

Borrows from DB row buffer. Zero allocation for core fields. Metadata is always owned (small data — UUIDs, timestamps).

```rust
pub struct PersistedEnvelope<'a, M = ()> {
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    payload: &'a [u8],
    metadata: M,               // owned — small data
}
```

All fields private. Construction controlled by nexus-store internals. Adapters return raw parts via trait methods, nexus-store assembles the envelope.

### No UpcastedEnvelope

Upcasters return raw `(String, u32, Vec<u8>)` — new event type, schema version, and payload. The `EventStore` facade holds these as temporaries and passes `(&str, &[u8])` to the codec. No third envelope type needed.

## Traits

### Codec (pluggable serialization)

Pure bytes. Knows nothing about envelopes, streams, versions, or metadata. Monomorphized via type parameter for zero-cost at call site.

```rust
pub trait Codec: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error>;
    fn decode<E: DomainEvent>(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
```

Users implement this for their chosen format (serde_json, postcard, musli, protobuf, rkyv, etc.).

### EventUpcaster (schema evolution)

Operates on raw bytes BEFORE codec deserialization. Transforms old event schemas to new ones without needing old Rust types.

```rust
pub trait EventUpcaster: Send + Sync {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;
    fn upcast(&self, event_type: &str, schema_version: u32, payload: &[u8]) -> (String, u32, Vec<u8>);
}
```

Upcasters are chained: V1 -> V2 -> V3. Applied in order during reads. Writes always use current schema version.

### EventStream<M> (GAT lending cursor)

Zero-allocation streaming. Each envelope borrows from the cursor's internal row buffer. The GAT lifetime ties the envelope to the cursor — previous envelope must be dropped before calling `next()` again.

```rust
pub trait EventStream<M = ()> {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_, M>, Self::Error>>;
}
```

### RawEventStore<M> (what adapters implement)

Bytes in, bytes out. Knows nothing about typed events or codecs. Adapters implement this for their database.

```rust
pub trait RawEventStore<M = ()>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a where Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<M>],
    ) -> Result<(), Self::Error>;

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error>;
}
```

`Stream<'a>` is a GAT — borrows from the store (DB connection). Enables the zero-allocation lending pattern.

## Facade

### EventStore<S, C, M> (what users call)

Composes RawEventStore + Codec + Upcasters. Users see typed events, never envelopes.

```rust
pub struct EventStore<S: RawEventStore<M>, C: Codec, M = ()> {
    raw: S,
    codec: C,
    upcasters: Vec<Box<dyn EventUpcaster>>,
}

impl<S: RawEventStore<M>, C: Codec, M> EventStore<S, C, M> {
    pub fn new(raw: S, codec: C) -> Self;
    pub fn with_upcaster(self, upcaster: impl EventUpcaster + 'static) -> Self;

    pub async fn load<A: Aggregate>(
        &self,
        id: &A::Id,
    ) -> Result<AggregateRoot<A>, StoreError>;

    pub async fn save<A: Aggregate>(
        &self,
        id: &A::Id,
        expected_version: Version,
        events: &[VersionedEvent<EventOf<A>>],
    ) -> Result<(), StoreError>;
}
```

Two methods. Load and save. That's it. Projections are a separate concern.

## Data Flow

### Save (write path)

```
User calls: store.save(&id, version, &events)
    → for each event:
        event_type = event.name()              (&'static str, free)
        payload = codec.encode(&event)          (Vec<u8>, one alloc)
        PendingEnvelope constructed              (pub(crate))
    → raw.append(stream_id, expected_version, &envelopes)
    → adapter writes to DB
```

### Load (read path)

```
User calls: store.load::<MyAggregate>(&id)
    → raw.read_stream(stream_id, Version::INITIAL)
    → returns Stream (GAT cursor)
    → for each PersistedEnvelope:
        if upcaster.can_upcast(event_type, schema_version):
            (new_type, new_ver, new_payload) = upcaster.upcast(...)
            event = codec.decode(&new_type, &new_payload)     (allocated)
        else:
            event = codec.decode(event_type, payload)          (zero alloc envelope)
        aggregate.apply_event(event)
        envelope dropped, cursor advances
    → return rehydrated AggregateRoot<A>
```

## Performance Characteristics

| Operation | Allocations per event |
|-----------|----------------------|
| Save | 1 (payload Vec<u8>) |
| Load (no upcasting) | 0 (borrows from row) + codec decode |
| Load (with upcasting) | 2 (new event_type String + new payload Vec<u8>) + codec decode |

## Crate Dependencies

- `nexus` — kernel types (Version, DomainEvent, Aggregate, AggregateRoot, EventOf, VersionedEvent)
- `thiserror` — error types

No async runtime dependency. Traits use `async fn` (Rust 1.75+ RPITIT). Adapters bring the runtime.

## Metadata

User-defined `M` is a type parameter on the store. All events share the same metadata shape. The metadata maps to additional DB columns.

```rust
// Bare minimum — no extra columns
let store = EventStore::new(raw, codec);  // M = ()

// Rich metadata — extra columns in DB
struct MyMetadata {
    correlation_id: Uuid,
    causation_id: Uuid,
    tenant_id: String,
    created_at: DateTime<Utc>,
}
let store: EventStore<_, _, MyMetadata> = EventStore::new(raw, codec);
```
