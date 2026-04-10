# Aggregate Snapshot Design

## Problem

Aggregates with thousands of events reload slowly because every event must be replayed from the beginning. Snapshots periodically persist aggregate state so rehydration only replays events *after* the snapshot.

## When to Use Snapshots

**Measure first.** Snapshots add complexity — only enable them when replay cost is measurable.

| Event Count | Snapshot Value |
|-------------|---------------|
| < 50 | Always hurts — overhead exceeds benefit |
| 50–200 | Rarely helps unless deserialization is expensive |
| 200–500 | Measure. Depends on event size and apply complexity |
| 500+ | Likely helps |
| 10,000+ | Essential — but also consider "Closing the Books" pattern (#139) |

**Preferred alternative:** Short-lived streams with natural lifecycle boundaries (e.g., `CashierShift` instead of `CashRegister`) eliminate the need for snapshots entirely. See #139.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Snapshot serialization | External codec — reuses `Codec<S>` / `BorrowingCodec<S>` | Aggregates stay serialization-free, same pattern as events |
| Crate location | `nexus-store` | Same persistence layer as `RawEventStore` |
| Trigger ownership | Repository-owned, transparent to caller | Snapshots are an infrastructure optimization, not business logic |
| Trait surface | Same `Repository<A>`, smarter implementation | Callers don't know or care about snapshots |
| Schema evolution | Versioned snapshots with explicit invalidation | Explicit version mismatch over silent deserialization failure |
| Read path | Owned `PersistedSnapshot` | Simpler than GAT holder; borrowing variant can be added later when benchmarks justify it |
| No-op | `()` implements `SnapshotStore` | Same pattern as `()` upcaster; compiler eliminates dead code |
| Feature gating | `snapshot`, `snapshot-json` features | Independent from event serialization features |
| Trigger type | Generic parameter `T` (not `Box<dyn>`) | Zero-cost monomorphization — compiler inlines `should_snapshot()` |
| Trigger signature | Receives old + new version + event names | Fixes batch-crossing bug; enables semantic triggers |
| Lazy snapshotting | Configurable snapshot-on-read | For write-heavy / IoT workloads where write-path overhead matters |
| Retention | `delete_snapshot` on `SnapshotStore` | Prevents unbounded growth on disk-constrained embedded targets |

## Audit Changes (2026-04-10)

Based on competitive analysis of Axon 5, Pekko, Marten, Equinox, eventsourced-rs, cqrs-es, and disintegrate:

| Issue | Resolution |
|-------|------------|
| **Critical: modulo trigger bug** | `SnapshotTrigger::should_snapshot` now receives `old_version` + `new_version` + `event_names`; `EveryNEvents` uses boundary-crossing check instead of modulo |
| **High: GAT Holder complexity** | Simplified to owned `PersistedSnapshot` struct; borrowing variant deferred |
| **High: `Box<dyn SnapshotTrigger>`** | Trigger is now a type parameter `T` on `Snapshotting` — monomorphized, zero-cost |
| **Medium: event-type triggers** | `AfterEventTypes` trigger added; trigger signature includes `event_names: &[&str]` |
| **Medium: lazy snapshotting** | Snapshotting facade supports snapshot-on-read after full replay |
| **Low: snapshot retention** | `delete_snapshot` added to `SnapshotStore` trait |
| **Informational** | GitHub issues created: #139 (Closing the Books), #140 (projection convergence), #141 (DurableState mode) |

## New Types and Traits

### SnapshotStore (byte-level, adapters implement)

```rust
pub trait SnapshotStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<Option<PersistedSnapshot>, Self::Error>> + Send;

    fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn delete_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

### PersistedSnapshot (owned read-path container)

```rust
pub struct PersistedSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
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

    async fn load_snapshot(&self, _id: &impl Id) -> Result<Option<PersistedSnapshot>, Infallible> {
        Ok(None)
    }

    async fn save_snapshot(&self, _id: &impl Id, _snapshot: &PendingSnapshot) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete_snapshot(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}
```

### SnapshotTrigger (pluggable strategy)

```rust
pub trait SnapshotTrigger: Send + Sync {
    /// Receives old/new version + event names to handle both boundary
    /// crossing and semantic triggers correctly.
    fn should_snapshot(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: &[&str],
    ) -> bool;
}

pub struct EveryNEvents(pub NonZeroU64);

impl SnapshotTrigger for EveryNEvents {
    fn should_snapshot(&self, old_version: Option<Version>, new_version: Version, _: &[&str]) -> bool {
        let n = self.0.get();
        let old_bucket = old_version.map_or(0, |v| v.as_u64() / n);
        let new_bucket = new_version.as_u64() / n;
        new_bucket > old_bucket
    }
}

pub struct AfterEventTypes { types: Vec<&'static str> }

impl SnapshotTrigger for AfterEventTypes {
    fn should_snapshot(&self, _: Option<Version>, _: Version, event_names: &[&str]) -> bool {
        event_names.iter().any(|name| self.types.iter().any(|t| t == name))
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
snapshot-json = ["snapshot", "json"]

# nexus-fjall/Cargo.toml
[features]
snapshot = ["nexus-store/snapshot"]
```

- `snapshot` — enables traits, types, builder methods, load/save logic
- `snapshot-json` — adds `JsonCodec` as default snapshot codec (independent from event `json` feature)
- Without `snapshot`, `EventStore` stays `EventStore<S, C, U>` — zero overhead

## Snapshotting Decorator

```rust
/// Trigger is a type param T — monomorphized, zero-cost.
pub struct Snapshotting<R, SS, SC, T> {
    inner: R,             // EventStore or ZeroCopyEventStore
    snapshot_store: SS,
    snapshot_codec: SC,
    trigger: T,
    schema_version: u32,
    snapshot_on_read: bool,
}
```

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

// Custom snapshot codec + semantic trigger + lazy on-read
let repo = store.repository()
    .snapshot_store(fjall_snapshots)
    .snapshot_codec(RkyvCodec)
    .snapshot_trigger(AfterEventTypes::new(&["OrderCompleted"]))
    .snapshot_on_read(true)
    .build();
```

Builder enforces via typestate:
- `snapshot_store` set without codec → blocked unless a default feature (`snapshot-json`) provides one
- `snapshot_trigger` defaults to `EveryNEvents(100)` if not specified
- `snapshot_schema_version` defaults to 1 if not specified
- `snapshot_on_read` defaults to false

## Load Path

```
load(id)
  │
  ├─ try load_snapshot(id)
  │   ├─ Ok(Some(snap)) AND snap.schema_version() == current →
  │   │    decode state from snap.payload() via snapshot codec
  │   │    create AggregateRoot::restore(id, state, snap.version())
  │   │    replay_from(root, snap.version().next())
  │   │    return root
  │   │
  │   └─ None OR schema mismatch OR error →
  │        full replay from Version::INITIAL via inner.load(id)
  │        if snapshot_on_read → try_save_snapshot (best-effort)
  │        return root
  │
  └─ return AggregateRoot<A>
```

Snapshot load failure (IO/decode) = cache miss, fall back to full replay.

## Save Path

```
save(aggregate, events)
  │
  ├─ old_version = aggregate.version()
  ├─ inner.save(aggregate, events) — encode + append + advance
  │
  ├─ trigger.should_snapshot(old_version, new_version, event_names)?
  │   ├─ yes → encode state via snapshot codec
  │   │         save_snapshot(id, PendingSnapshot { version, schema_version, payload })
  │   │         (best-effort — errors silently ignored)
  │   └─ no  → skip
  │
  └─ return Ok(())
```

Snapshot save is best-effort — happens after successful event append. Events are the source of truth.

## Error Handling

- **Snapshot load failure:** cache miss, full replay. Errors silently discarded.
- **Snapshot save failure:** silently discarded. Events already persisted.
- **Schema version mismatch:** expected after state changes. Silent fallback to full replay.
- No new variants on `StoreError` — snapshots are transparent and best-effort.

## Fjall Adapter

Third partition in `FjallStore`, point-read optimized (not scan-optimized like events).

Key: aggregate stream ID (numeric u64).
Value: `[u32 LE schema_version][u64 BE version][payload]`.

Implements `SnapshotStore` behind `nexus-fjall/snapshot` feature.

## What Doesn't Change

- `Repository<A>` trait signature
- `AggregateState` trait (kernel crate)
- `RawEventStore` trait
- Event codecs, upcasters
- Callers of `repository.load()` / `repository.save()`

## Kernel Addition

- `AggregateRoot::restore(id, state, version)` — new constructor for creating an aggregate from snapshot state. Minimal change: no new traits, no new bounds.

## Performance Considerations

**Snapshot overhead:** Each snapshot adds one read (load) and one write (save) per aggregate lifecycle. On the read path, a snapshot miss falls back to full replay with zero additional cost beyond the failed point-read. On the write path, snapshot saves are best-effort — failures never block event persistence.

**When snapshots hurt:**
- **Small aggregates (< 50 events):** The snapshot read + deserialization cost exceeds replaying 50 events. The crossover point depends on event size and `apply()` complexity.
- **Write-heavy workloads:** Use `snapshot_on_read(true)` (lazy snapshotting) to move snapshot creation to the read path, avoiding write amplification.
- **Rapidly-evolving state schemas:** Each schema version change invalidates all existing snapshots, forcing full replay until new snapshots are built. Frequent schema changes negate snapshot benefits.

**When snapshots help:**
- **10,000+ events per aggregate:** Essential. Even with efficient `apply()` and fast storage, deserializing 10K events measurably impacts latency.
- **Expensive deserialization:** Aggregates with large event payloads or complex `apply()` logic benefit from snapshots at lower event counts.
- **Latency-sensitive reads:** IoT/mobile scenarios where reload latency must be bounded regardless of aggregate history length.

**Alternative: "Closing the Books" (#139):** Design aggregates with natural lifecycle boundaries (e.g., `CashierShift` per shift vs. `CashRegister` forever). When an aggregate completes its lifecycle, archive its stream and start a new one. This eliminates the need for snapshots entirely and is the preferred approach when domain semantics allow it.

## Related Issues

- #139 — Document "Closing the Books" pattern as preferred alternative
- #140 — Design snapshot traits for future projection convergence
- #141 — Consider DurableState mode (snapshot-only persistence, Tier 3)
