# nexus-store Design

**Date:** 2026-04-02
**Status:** Approved
**Scope:** The edge layer between kernel aggregates and databases

## Goals

1. **Zero-allocation rehydration** via GAT lending cursor
2. **Pluggable serialization** — codec as type parameter (monomorphized)
3. **Pluggable schema evolution** — upcasters transform raw bytes before deserialization
4. **Customizable envelope** — users define their own metadata
5. **Two read paths** — lending cursor for rehydration, owned stream for projections

## Non-Goals

- No built-in schema registry (provide the port, not the implementation)
- No default serialization format (users bring their codec)
- No actual persistence (adapters like nexus-rusqlite handle that)

## Architecture

```
User code
    ↓
EventStore (typed facade)
    ↓ encode/decode via Codec
    ↓ upcast via EventUpcaster
    ↓
RawEventStore (adapter trait — bytes in/out)
    ↓
Database (SQLite, fjall, Postgres, etc.)
```

```
Write: typed event → Codec::encode() → EventEnvelope → RawEventStore::append()
Read:  RawEventStore::read() → EventEnvelope → EventUpcaster::upcast() → Codec::decode() → typed event
```

## Core Types

### EventEnvelope (borrowed — zero allocation)

Used during rehydration. Borrows from the database cursor row buffer.

```rust
pub struct EventEnvelope<'a, M = ()> {
    pub stream_id: &'a str,
    pub version: Version,
    pub event_type: &'a str,
    pub payload: &'a [u8],
    pub metadata: M,
}
```

Four required fields (map to DB columns):
- `stream_id` — which aggregate's event stream
- `version` — position in the stream (from kernel's Version)
- `event_type` — which event variant (from DomainEvent::name())
- `payload` — serialized event bytes

`metadata: M` is user-defined. Could be `()` (bare minimum) or a rich struct with correlation_id, causation_id, tenant_id, timestamp, user_id, trace_id, etc.

### OwnedEventEnvelope (owned — for async-per-event)

Used for projections, subscriptions, and sending across async boundaries.

```rust
pub struct OwnedEventEnvelope<M = ()> {
    pub stream_id: Arc<str>,
    pub version: Version,
    pub event_type: &'static str,
    pub payload: Vec<u8>,
    pub metadata: M,
}
```

- `stream_id: Arc<str>` — shared across events in same stream (one allocation)
- `event_type: &'static str` — from DomainEvent::name() (zero allocation)
- `payload: Vec<u8>` — unavoidable (bytes must live somewhere)

### Codec (pluggable serialization)

Monomorphized via type parameter — zero-cost at the call site.

```rust
pub trait Codec: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error>;
    fn decode<E: DomainEvent>(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
```

Users implement this for their chosen format (serde_json, postcard, musli, protobuf, rkyv, etc.). The store calls `codec.encode()` on write and `codec.decode()` on read.

`event_type` is passed to `decode()` so the codec knows which variant to deserialize — the enum variant name is stored alongside the payload.

### EventUpcaster (schema evolution)

Operates on raw bytes BEFORE the codec deserializes. Transforms old event formats to new ones without needing the old Rust types.

```rust
pub trait EventUpcaster: Send + Sync {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;
    fn upcast(&self, event_type: &str, schema_version: u32, payload: &[u8]) -> UpcasterResult;
}

pub enum UpcasterResult {
    /// Event was transformed — new payload and optionally new type/version
    Upcasted {
        event_type: String,
        schema_version: u32,
        payload: Vec<u8>,
    },
    /// Event does not need upcasting — pass through unchanged
    Unchanged,
}
```

Upcasters are chained: V1 → V2 → V3. The store applies them in order during reads. Writes always use the current schema version.

### EventStream (GAT lending cursor — rehydration)

Zero-allocation streaming. Each envelope borrows from the cursor's internal row buffer.

```rust
pub trait EventStream {
    type Envelope<'a>: 'a where Self: 'a;
    type Error;

    async fn next(&mut self) -> Option<Result<Self::Envelope<'_>, Self::Error>>;
}
```

Adapters implement this by wrapping their database cursor. Each call to `next()` advances the cursor and returns a borrowed envelope. The previous envelope must be dropped before calling `next()` again — enforced by the GAT lifetime.

Usage during rehydration:

```rust
let mut cursor = store.open_cursor::<UserAggregate>(&id).await?;
let mut root = AggregateRoot::<UserAggregate>::new(id);
while let Some(envelope) = cursor.next().await {
    let envelope = envelope?;
    let event = codec.decode(envelope.event_type, envelope.payload)?;
    root.apply_event(event);
    // envelope dropped here — cursor buffer reused
}
```

### RawEventStore (adapter trait)

What database adapters implement. Bytes in, bytes out. Knows nothing about typed events or codecs.

```rust
pub trait RawEventStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Cursor<'a>: EventStream<Error = Self::Error> + 'a where Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[EventEnvelope<'_>],
    ) -> Result<(), Self::Error>;

    async fn open_cursor(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Cursor<'_>, Self::Error>;
}
```

`Cursor` is a GAT — it borrows from the store (and transitively from the DB connection). This enables the zero-allocation lending pattern.

### EventStore (typed facade)

Composes RawEventStore + Codec + Upcasters. The user-facing API.

```rust
pub struct EventStore<S: RawEventStore, C: Codec, M = ()> {
    raw: S,
    codec: C,
    upcasters: Vec<Box<dyn EventUpcaster>>,
    _metadata: PhantomData<M>,
}

impl<S: RawEventStore, C: Codec, M> EventStore<S, C, M> {
    pub fn new(raw: S, codec: C) -> Self;
    pub fn with_upcaster(mut self, upcaster: impl EventUpcaster + 'static) -> Self;

    pub async fn append<A: Aggregate>(
        &self,
        id: &A::Id,
        expected_version: Version,
        events: &[VersionedEvent<EventOf<A>>],
    ) -> Result<(), StoreError>;

    pub async fn load<A: Aggregate>(
        &self,
        id: &A::Id,
    ) -> Result<AggregateRoot<A>, StoreError>;
}
```

`load()` internally opens a cursor, decodes each event (with upcasting), applies to aggregate, and returns the fully rehydrated `AggregateRoot<A>`.

### EventSubscription (owned stream — projections)

For async-per-event processing (projections, reactions, read model updates).

```rust
pub trait EventSubscription {
    type Error;

    async fn next(&mut self) -> Option<Result<OwnedEventEnvelope, Self::Error>>;
}
```

Returns `OwnedEventEnvelope` — fully owned, can cross async boundaries, can be stored, can be sent to other tasks.

## Two Read Paths

| Use Case | API | Allocation | Envelope Type |
|----------|-----|------------|---------------|
| Rehydration | `EventStream` (GAT cursor) | Zero per event | `EventEnvelope<'a>` (borrowed) |
| Projections | `EventSubscription` | One per event | `OwnedEventEnvelope` (owned) |

These are not two ways to do the same thing. They are two different use cases with different performance characteristics.

## Dependency Graph

```
nexus (kernel)
    ↑
nexus-store (this crate)
    ↑
nexus-rusqlite / nexus-fjall (adapters)
```

nexus-store depends on nexus (kernel) for: `Version`, `DomainEvent`, `Aggregate`, `AggregateRoot`, `EventOf`, `VersionedEvent`.

nexus-store adds: `async` (tokio), serialization traits, envelope types, store traits.

## Crate Dependencies

- `nexus` — kernel types
- `thiserror` — error types
- `smallvec` — from kernel (transitive)

No serialization crate dependency. No database driver. No tokio (only the trait definitions — adapters bring the runtime).

## Envelope ↔ DB Schema Mapping

The envelope fields map directly to database columns:

```sql
-- Minimum schema (M = ())
CREATE TABLE events (
    stream_id   TEXT NOT NULL,
    version     INTEGER NOT NULL,
    event_type  TEXT NOT NULL,
    payload     BLOB NOT NULL,
    PRIMARY KEY (stream_id, version)
);

-- Rich schema (M = RichMetadata)
CREATE TABLE events (
    stream_id       TEXT NOT NULL,
    version         INTEGER NOT NULL,
    event_type      TEXT NOT NULL,
    payload         BLOB NOT NULL,
    correlation_id  TEXT,
    causation_id    TEXT,
    tenant_id       TEXT,
    created_at      TEXT NOT NULL,
    user_id         TEXT,
    PRIMARY KEY (stream_id, version)
);
```

The adapter generates the schema from the envelope + metadata type. Different metadata = different table schema.
