# Aggregate Snapshot Design

## Problem

Aggregates with thousands of events reload slowly because every event must be replayed from the beginning. Snapshots periodically persist aggregate state so rehydration only replays events *after* the snapshot.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Snapshot serialization | External codec — reuses `Codec<S>` / `BorrowingCodec<S>` | Aggregates stay serialization-free, same pattern as events |
| Crate location | `nexus-store` | Same persistence layer as `RawEventStore` |
| Trigger ownership | Repository-owned, transparent to caller | Snapshots are an infrastructure optimization, not business logic |
| Trait surface | Same `Repository<A>`, smarter implementation | Callers don't know or care about snapshots |
| Schema evolution | Versioned snapshots with explicit invalidation | Explicit version mismatch over silent deserialization failure |
| Read path | Borrowed via GAT holder (zero-copy capable) | Mirrors `PersistedEnvelope<'a>` / `EventStream` pattern |
| No-op | `()` implements `SnapshotStore` | Same pattern as `()` upcaster; compiler eliminates dead code |
| Feature gating | `snapshot`, `snapshot-json` features | Independent from event serialization features |

## New Types and Traits

### SnapshotStore (byte-level, adapters implement)

```rust
pub trait SnapshotStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Holder<'a>: SnapshotRead + 'a where Self: 'a;

    fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<Option<Self::Holder<'_>>, Self::Error>> + Send;

    fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

### SnapshotRead (borrowed read-path accessor)

```rust
pub trait SnapshotRead {
    fn version(&self) -> Version;
    fn schema_version(&self) -> u32;
    fn payload(&self) -> &[u8];
}
```

### PendingSnapshot (owned write-path container)

```rust
pub struct PendingSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}
```

### No-op implementation

```rust
impl SnapshotStore for () {
    type Error = Infallible;
    type Holder<'a> = /* never-constructed type */;

    async fn load_snapshot(&self, _id: &impl Id) -> Result<Option<Self::Holder<'_>>, Infallible> {
        Ok(None)
    }

    async fn save_snapshot(&self, _id: &impl Id, _snapshot: &PendingSnapshot) -> Result<(), Infallible> {
        Ok(())
    }
}
```

### SnapshotTrigger (pluggable strategy)

```rust
pub trait SnapshotTrigger: Send + Sync {
    fn should_snapshot(&self, events_since_snapshot: u64) -> bool;
}

pub struct EveryNEvents(pub NonZeroU64);

impl SnapshotTrigger for EveryNEvents {
    fn should_snapshot(&self, events_since_snapshot: u64) -> bool {
        events_since_snapshot >= self.0.get()
    }
}
```

### Snapshot codec — reuses existing traits

No new codec traits. Snapshots reuse `Codec<A::State>` and `BorrowingCodec<A::State>` directly.

## Feature Gates

```toml
# nexus-store/Cargo.toml
[features]
snapshot = []
snapshot-json = ["snapshot", "dep:serde", "dep:serde_json"]

# nexus-fjall/Cargo.toml
[features]
snapshot = ["nexus-store/snapshot"]
```

- `snapshot` — enables traits, types, builder methods, load/save logic
- `snapshot-json` — adds `JsonCodec` as default snapshot codec (independent from event `json` feature)
- Without `snapshot`, `EventStore` stays `EventStore<S, C, U>` — zero overhead

## Builder Integration

```rust
// Without snapshot feature — unchanged
let repo = store.repository().build();

// With snapshot-json — auto-wired default codec
let repo = store.repository()
    .snapshot_store(fjall_snapshots)
    .snapshot_trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
    .snapshot_schema_version(1)
    .build();

// Custom snapshot codec (e.g., rkyv for snapshots, JSON for events)
let repo = store.repository()
    .snapshot_store(fjall_snapshots)
    .snapshot_codec(RkyvCodec)
    .snapshot_trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
    .build();
```

Builder enforces via typestate:
- `snapshot_store` set without codec → blocked unless a default feature (`snapshot-json`) provides one
- `snapshot_trigger` defaults to `EveryNEvents(100)` if not specified
- `snapshot_schema_version` defaults to 1 if not specified

### EventStore with snapshot support

```rust
#[cfg(feature = "snapshot")]
pub struct EventStore<S, C, U = (), SS = (), SC = ()> {
    store: Store<S>,
    codec: C,
    upcaster: U,
    snapshot_store: SS,
    snapshot_codec: SC,
    snapshot_trigger: Box<dyn SnapshotTrigger>,
    snapshot_schema_version: u32,
}

#[cfg(not(feature = "snapshot"))]
pub struct EventStore<S, C, U = ()> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}
```

Same pattern applies to `ZeroCopyEventStore`.

## Load Path

```
load(id)
  │
  ├─ snapshot store != () ?
  │   ├─ yes → load_snapshot(id)
  │   │         ├─ Some(holder) AND holder.schema_version() == current →
  │   │         │    decode state from holder.payload() via snapshot codec
  │   │         │    create AggregateRoot with state + holder.version()
  │   │         │    read_stream(id, from: holder.version().next())
  │   │         │    replay remaining events only
  │   │         │
  │   │         └─ None OR schema mismatch →
  │   │              full replay from Version::INITIAL (same as today)
  │   │
  │   └─ no → full replay from Version::INITIAL (same as today)
  │
  └─ return AggregateRoot<A>
```

Snapshot load failure (IO/decode) = cache miss, fall back to full replay. Log warning.

## Save Path

```
save(aggregate, events)
  │
  ├─ encode + append events (same as today)
  │
  ├─ snapshot store != () ?
  │   ├─ yes → compute events_since_snapshot =
  │   │         current_version - snapshot_version (or current_version if no snapshot)
  │   │         trigger.should_snapshot(events_since_snapshot)?
  │   │           ├─ yes → encode state via snapshot codec
  │   │           │         save_snapshot(id, PendingSnapshot { version, schema_version, payload })
  │   │           └─ no  → skip
  │   └─ no → skip
  │
  ├─ advance_version + apply_events (same as today)
  │
  └─ return Ok(())
```

Snapshot save is best-effort — happens after successful event append. Failure is logged, not propagated. Events are the source of truth.

## Error Handling

- **Snapshot load failure:** cache miss, full replay. Log warning.
- **Snapshot save failure:** log warning, don't propagate.
- **Schema version mismatch:** expected after state changes. Silent fallback to full replay.
- No new variants on `StoreError` — snapshots are transparent and best-effort.

## Fjall Adapter

Third partition in `FjallStore`, point-read optimized (not scan-optimized like events).

Key: aggregate stream ID (numeric u64).
Value: `[u32 LE schema_version][u64 BE version][payload]`.

Implements `SnapshotStore` behind `nexus-fjall/snapshot` feature.

## What Doesn't Change

- `Repository<A>` trait signature
- `Aggregate`, `AggregateState`, `AggregateRoot` (kernel crate)
- `RawEventStore` trait
- Event codecs, upcasters
- Callers of `repository.load()` / `repository.save()`
