# Aggregate Snapshot Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add feature-gated aggregate snapshot support to nexus-store so rehydration skips replaying the full event stream.

**Architecture:** `Snapshotting<R, SS, SC, T>` decorator wraps `EventStore`/`ZeroCopyEventStore`, intercepts `load()`/`save()` to check/write snapshots. Extract shared replay logic into a `pub(crate)` trait `ReplayFrom<A>` to avoid duplicating the stream replay loop. All snapshot types live behind `#[cfg(feature = "snapshot")]`. `()` serves as no-op `SnapshotStore`. Builder typestate gains a `Snap` param (`NoSnapshot` or `WithSnapshot<SS, SC, T>`) to produce either plain `EventStore` or `Snapshotting<EventStore, SS, SC, T>`.

**Tech Stack:** Rust, nexus kernel (`AggregateRoot`), nexus-store traits, fjall LSM storage.

**Audit applied:** Plan revised based on competitive analysis of Axon 5, Pekko, Marten, Equinox, eventsourced-rs, cqrs-es, disintegrate. See `docs/plans/2026-04-10-snapshot-design.md` for design decisions.

---

## Key Changes from Audit

| Issue | Resolution |
|-------|------------|
| **Critical: modulo trigger bug** | `SnapshotTrigger::should_snapshot` now receives `old_version` + `new_version` + `event_names`; `EveryNEvents` uses boundary-crossing check instead of modulo |
| **High: GAT Holder complexity** | Simplified to owned `PersistedSnapshot` struct; borrowing variant deferred to when benchmarks justify it |
| **High: `Box<dyn SnapshotTrigger>`** | Trigger is now a type parameter `T` on `Snapshotting` — monomorphized, zero-cost |
| **Medium: event-type triggers** | `AfterEventTypes` trigger added; trigger signature includes `event_names: &[&str]` |
| **Medium: lazy snapshotting** | Snapshotting facade supports snapshot-on-read (after full replay exceeds threshold) |
| **Low: snapshot retention** | `delete_snapshot` added to `SnapshotStore` trait |
| **Low: performance docs** | Guidance on when snapshots help vs. hurt included in design doc |

---

### Task 1: Add feature flags to nexus-store

**Files:**
- Modify: `crates/nexus-store/Cargo.toml`

**Step 1: Add snapshot features**

Add to the `[features]` section:

```toml
[features]
testing = ["dep:tokio"]
bounded-labels = []
serde = ["dep:serde"]
json = ["serde", "dep:serde_json"]
snapshot = []
snapshot-json = ["snapshot", "json"]
```

**Step 2: Verify it compiles**

Run: `cargo build -p nexus-store` and `cargo build -p nexus-store --features snapshot`
Expected: both succeed (features exist but nothing uses them yet)

**Step 3: Commit**

```
feat(store): add snapshot and snapshot-json feature flags
```

---

### Task 2: Create PendingSnapshot and PersistedSnapshot structs

**Files:**
- Create: `crates/nexus-store/src/snapshot/pending.rs`
- Create: `crates/nexus-store/src/snapshot/persisted.rs`
- Create: `crates/nexus-store/src/snapshot/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Create `crates/nexus-store/tests/snapshot_tests.rs`:

```rust
#![cfg(feature = "snapshot")]

use nexus::Version;
use nexus_store::snapshot::{PendingSnapshot, PersistedSnapshot};

#[test]
fn pending_snapshot_stores_version_and_payload() {
    let version = Version::new(42).unwrap();
    let payload = vec![1, 2, 3];
    let snap = PendingSnapshot::new(version, 1, payload.clone());

    assert_eq!(snap.version(), version);
    assert_eq!(snap.schema_version(), 1);
    assert_eq!(snap.payload(), &payload);
}

#[test]
fn pending_snapshot_schema_version_must_be_nonzero() {
    let version = Version::new(1).unwrap();
    let result = PendingSnapshot::try_new(version, 0, vec![]);
    assert!(result.is_err());
}

#[test]
fn persisted_snapshot_stores_version_and_payload() {
    let version = Version::new(10).unwrap();
    let snap = PersistedSnapshot::new(version, 2, vec![4, 5, 6]);

    assert_eq!(snap.version(), version);
    assert_eq!(snap.schema_version(), 2);
    assert_eq!(snap.payload(), &[4, 5, 6]);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store --features snapshot -- snapshot_tests`
Expected: FAIL — module `snapshot` doesn't exist

**Step 3: Create the snapshot module**

Create `crates/nexus-store/src/snapshot/mod.rs`:

```rust
mod pending;
mod persisted;

pub use pending::PendingSnapshot;
pub use persisted::PersistedSnapshot;
```

Create `crates/nexus-store/src/snapshot/pending.rs`:

```rust
use nexus::Version;

use crate::error::InvalidSchemaVersion;

/// Snapshot payload ready for persistence (write path).
///
/// Contains the serialized aggregate state, the aggregate version at
/// snapshot time, and a schema version for invalidation.
#[derive(Debug, Clone)]
pub struct PendingSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl PendingSnapshot {
    /// Create a new pending snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0. Use [`try_new`](Self::try_new)
    /// for a fallible alternative.
    #[must_use]
    pub fn new(version: Version, schema_version: u32, payload: Vec<u8>) -> Self {
        match Self::try_new(version, schema_version, payload) {
            Ok(snap) => snap,
            Err(e) => panic!("{e}"),
        }
    }

    /// Try to create a new pending snapshot.
    ///
    /// Returns `Err` if `schema_version` is 0.
    pub fn try_new(
        version: Version,
        schema_version: u32,
        payload: Vec<u8>,
    ) -> Result<Self, InvalidSchemaVersion> {
        if schema_version == 0 {
            return Err(InvalidSchemaVersion);
        }
        Ok(Self {
            version,
            schema_version,
            payload,
        })
    }

    /// The aggregate version at the time of the snapshot.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
```

Create `crates/nexus-store/src/snapshot/persisted.rs`:

```rust
use nexus::Version;

use crate::error::InvalidSchemaVersion;

/// Persisted snapshot loaded from storage (read path).
///
/// Owns the payload bytes. This is the simplified owned variant —
/// a borrowing variant with GAT lifetimes can be added later if
/// benchmarks show the extra allocation matters for specific backends.
#[derive(Debug, Clone)]
pub struct PersistedSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl PersistedSnapshot {
    /// Create a new persisted snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0.
    #[must_use]
    pub fn new(version: Version, schema_version: u32, payload: Vec<u8>) -> Self {
        match Self::try_new(version, schema_version, payload) {
            Ok(snap) => snap,
            Err(e) => panic!("{e}"),
        }
    }

    /// Try to create a new persisted snapshot.
    pub fn try_new(
        version: Version,
        schema_version: u32,
        payload: Vec<u8>,
    ) -> Result<Self, InvalidSchemaVersion> {
        if schema_version == 0 {
            return Err(InvalidSchemaVersion);
        }
        Ok(Self {
            version,
            schema_version,
            payload,
        })
    }

    /// The aggregate version at the time of the snapshot.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(feature = "snapshot")]
pub mod snapshot;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-store --features snapshot -- snapshot_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add PendingSnapshot and PersistedSnapshot types
```

---

### Task 3: Create SnapshotStore trait + () no-op

**Files:**
- Create: `crates/nexus-store/src/snapshot/store.rs`
- Modify: `crates/nexus-store/src/snapshot/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Add to `snapshot_tests.rs`:

```rust
use nexus_store::snapshot::SnapshotStore;

#[tokio::test]
async fn unit_snapshot_store_returns_none() {
    let store = ();
    let id = String::from("agg-1");
    let result = store.load_snapshot(&id).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn unit_snapshot_store_save_succeeds() {
    let store = ();
    let id = String::from("agg-1");
    let snap = PendingSnapshot::new(Version::new(1).unwrap(), 1, vec![1, 2, 3]);
    let result = store.save_snapshot(&id, &snap).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn unit_snapshot_store_delete_succeeds() {
    let store = ();
    let id = String::from("agg-1");
    let result = store.delete_snapshot(&id).await;
    assert!(result.is_ok());
}
```

**Step 2: Run test — expected FAIL**

**Step 3: Create SnapshotStore trait**

Create `crates/nexus-store/src/snapshot/store.rs`:

```rust
use std::convert::Infallible;
use std::future::Future;

use nexus::Id;

use super::pending::PendingSnapshot;
use super::persisted::PersistedSnapshot;

/// Byte-level snapshot storage trait.
///
/// Adapters (fjall, postgres, etc.) implement this to persist and
/// retrieve serialized aggregate state.
///
/// `()` is the no-op implementation: `load_snapshot` always returns
/// `None`, `save_snapshot` and `delete_snapshot` silently discard.
/// Used when snapshot support is not configured.
pub trait SnapshotStore: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the most recent snapshot for the given aggregate.
    ///
    /// Returns `None` if no snapshot exists.
    fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<Option<PersistedSnapshot>, Self::Error>> + Send;

    /// Persist a snapshot for the given aggregate.
    ///
    /// Overwrites any existing snapshot for this aggregate.
    fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete the snapshot for the given aggregate.
    ///
    /// Returns `Ok(())` if no snapshot exists (idempotent).
    fn delete_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — snapshots disabled
// ═══════════════════════════════════════════════════════════════════════════

impl SnapshotStore for () {
    type Error = Infallible;

    async fn load_snapshot(
        &self,
        _id: &impl Id,
    ) -> Result<Option<PersistedSnapshot>, Infallible> {
        Ok(None)
    }

    async fn save_snapshot(
        &self,
        _id: &impl Id,
        _snapshot: &PendingSnapshot,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete_snapshot(
        &self,
        _id: &impl Id,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}
```

Update `snapshot/mod.rs`:

```rust
mod pending;
mod persisted;
mod store;

pub use pending::PendingSnapshot;
pub use persisted::PersistedSnapshot;
pub use store::SnapshotStore;
```

**Step 4: Run tests — expected PASS**

**Step 5: Commit**

```
feat(store): add SnapshotStore trait with () no-op and delete_snapshot
```

---

### Task 4: Create SnapshotTrigger trait + EveryNEvents + AfterEventTypes

**Files:**
- Create: `crates/nexus-store/src/snapshot/trigger.rs`
- Modify: `crates/nexus-store/src/snapshot/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Add to `snapshot_tests.rs`:

```rust
use std::num::NonZeroU64;
use nexus_store::snapshot::{SnapshotTrigger, EveryNEvents, AfterEventTypes};

// ── EveryNEvents ────────────────────────────────────────────────

#[test]
fn every_n_events_triggers_on_boundary_crossing() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Single-event saves crossing the boundary
    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_snapshot(v99, v100, &[]));

    // Not yet at boundary
    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_snapshot(v98, v99_ver, &[]));
}

#[test]
fn every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Batch of 7 events crossing the 100 boundary: 96 → 103
    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_snapshot(old, new, &[]));
}

#[test]
fn every_n_events_first_save_triggers_at_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Fresh aggregate, first save crosses boundary
    let new = Version::new(100).unwrap();
    assert!(trigger.should_snapshot(None, new, &[]));

    // Fresh aggregate, first save below boundary
    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_snapshot(None, new_50, &[]));
}

#[test]
fn every_1_event_always_triggers() {
    let trigger = EveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_snapshot(None, Version::new(1).unwrap(), &[]));
    assert!(trigger.should_snapshot(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        &[],
    ));
}

// ── AfterEventTypes ─────────────────────────────────────────────

#[test]
fn after_event_types_triggers_on_matching_event() {
    let trigger = AfterEventTypes::new(&["OrderCompleted", "OrderCancelled"]);

    assert!(trigger.should_snapshot(None, Version::new(5).unwrap(), &["OrderCompleted"]));
    assert!(trigger.should_snapshot(None, Version::new(5).unwrap(), &["OrderCancelled"]));
    assert!(!trigger.should_snapshot(None, Version::new(5).unwrap(), &["ItemAdded"]));
}

#[test]
fn after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_snapshot(
        None,
        Version::new(5).unwrap(),
        &["ItemAdded", "OrderCompleted"],
    ));
}

#[test]
fn after_event_types_does_not_trigger_on_empty_events() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_snapshot(None, Version::new(5).unwrap(), &[]));
}
```

**Step 2: Run test — expected FAIL**

**Step 3: Create trigger module**

Create `crates/nexus-store/src/snapshot/trigger.rs`:

```rust
use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to take a snapshot.
///
/// Checked after each successful `save()`. The trigger receives both the
/// old and new aggregate version (to detect boundary crossings regardless
/// of batch size) and the names of events just persisted (for semantic
/// triggers).
pub trait SnapshotTrigger: Send + Sync {
    /// Whether a snapshot should be taken after this save.
    ///
    /// - `old_version`: aggregate version before save (`None` for new aggregates)
    /// - `new_version`: aggregate version after save
    /// - `event_names`: names of events just persisted (from `DomainEvent::name()`)
    fn should_snapshot(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: &[&str],
    ) -> bool;
}

/// Trigger a snapshot every N events.
///
/// Detects when a save crosses an N-event boundary, regardless of batch
/// size. For example, `EveryNEvents(100)` triggers when the aggregate
/// crosses version 100, 200, 300, etc. — even if the batch was 7 events
/// jumping from version 96 to 103.
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl SnapshotTrigger for EveryNEvents {
    fn should_snapshot(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        _event_names: &[&str],
    ) -> bool {
        let n = self.0.get();
        let old_bucket = old_version.map_or(0, |v| v.as_u64() / n);
        let new_bucket = new_version.as_u64() / n;
        new_bucket > old_bucket
    }
}

/// Trigger a snapshot after specific event types are persisted.
///
/// Useful for domain milestones (e.g., `OrderCompleted`, `AccountClosed`)
/// where the aggregate is unlikely to change further, or where a stable
/// snapshot point aligns with business semantics.
#[derive(Debug, Clone)]
pub struct AfterEventTypes {
    types: Vec<&'static str>,
}

impl AfterEventTypes {
    /// Create a trigger that fires when any of the given event types is persisted.
    #[must_use]
    pub fn new(types: &[&'static str]) -> Self {
        Self {
            types: types.to_vec(),
        }
    }
}

impl SnapshotTrigger for AfterEventTypes {
    fn should_snapshot(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        event_names: &[&str],
    ) -> bool {
        event_names
            .iter()
            .any(|name| self.types.iter().any(|t| t == name))
    }
}
```

Update `snapshot/mod.rs` to export `SnapshotTrigger`, `EveryNEvents`, and `AfterEventTypes`.

**Step 4: Run tests — expected PASS**

**Step 5: Commit**

```
feat(store): add SnapshotTrigger trait with EveryNEvents and AfterEventTypes
```

---

### Task 5: Add AggregateRoot::restore to kernel

**Files:**
- Modify: `crates/nexus/src/aggregate.rs`
- Test: `crates/nexus/tests/` (new or existing aggregate test file)

This is a **kernel change** — minimal, just a new constructor. No trait changes, no new bounds. Needed so the store can construct an `AggregateRoot` from a deserialized snapshot state.

**Step 1: Write the failing test**

Find the existing aggregate tests and add:

```rust
#[test]
fn restore_creates_root_at_given_state_and_version() {
    let id = TestId(1);
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let root = AggregateRoot::<TestAggregate>::restore(id.clone(), state, version);

    assert_eq!(root.id(), &id);
    assert_eq!(root.state().value, 42);
    assert_eq!(root.version(), Some(version));
}

#[test]
fn restore_then_replay_continues_from_snapshot_version() {
    let id = TestId(1);
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let mut root = AggregateRoot::<TestAggregate>::restore(id, state, version);

    // Next replay must be version 11
    let result = root.replay(Version::new(11).unwrap(), &CounterEvent::Incremented);
    assert!(result.is_ok());
    assert_eq!(root.state().value, 43);
    assert_eq!(root.version(), Some(Version::new(11).unwrap()));
}

#[test]
fn restore_then_replay_rejects_wrong_version() {
    let id = TestId(1);
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let mut root = AggregateRoot::<TestAggregate>::restore(id, state, version);

    // Replaying version 10 (not 11) must fail
    let result = root.replay(Version::new(10).unwrap(), &CounterEvent::Incremented);
    assert!(result.is_err());
}
```

Use whichever test aggregate types already exist in the kernel test files. Adapt names as needed.

**Step 2: Run test — expected FAIL**

Run: `cargo test -p nexus -- restore`
Expected: FAIL — no method `restore` on `AggregateRoot`

**Step 3: Add the restore constructor**

In `crates/nexus/src/aggregate.rs`, add to `impl<A: Aggregate> AggregateRoot<A>` (after `new`):

```rust
    /// Create an aggregate root restored from a snapshot.
    ///
    /// The root is initialized with the given state and version,
    /// as if those events had already been replayed. Subsequent
    /// calls to [`replay`](Self::replay) will expect versions
    /// starting at `version + 1`.
    ///
    /// Used by snapshot-aware repositories to skip full event replay.
    #[must_use]
    pub fn restore(id: A::Id, state: A::State, version: Version) -> Self {
        Self {
            id,
            state,
            version: Some(version),
        }
    }
```

**Step 4: Run tests — expected PASS**

Run: `cargo test -p nexus -- restore`
Expected: PASS

Also run full kernel tests: `cargo test -p nexus`
Expected: PASS (no regressions)

**Step 5: Commit**

```
feat(kernel): add AggregateRoot::restore for snapshot rehydration
```

---

### Task 6: Extract ReplayFrom trait from EventStore/ZeroCopyEventStore

**Files:**
- Create: `crates/nexus-store/src/store/replay.rs`
- Modify: `crates/nexus-store/src/store/event_store.rs`
- Modify: `crates/nexus-store/src/store/zero_copy_event_store.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`

This is a **refactor** — no behavior change. Extract the replay loop from both `load()` implementations into a shared `pub(crate)` trait so `Snapshotting` can call partial replay without duplicating code.

**Step 1: Run existing tests to establish baseline**

Run: `cargo test -p nexus-store --features json`
Expected: all PASS

**Step 2: Create the ReplayFrom trait**

Create `crates/nexus-store/src/store/replay.rs`:

```rust
use crate::error::StoreError;
use nexus::{Aggregate, AggregateRoot, Version};

/// Internal trait for replaying events from a given starting point.
///
/// Both `EventStore` and `ZeroCopyEventStore` implement this to share
/// replay logic with `Snapshotting`. Not public API.
pub(crate) trait ReplayFrom<A: Aggregate>: Send + Sync {
    /// Replay events from `from` version into `root`, returning
    /// the updated aggregate.
    fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> impl std::future::Future<Output = Result<AggregateRoot<A>, StoreError>> + Send;
}
```

**Step 3: Implement for EventStore**

In `event_store.rs`, add `use super::replay::ReplayFrom;` and implement:

```rust
impl<A, S, C, U> ReplayFrom<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: Upcaster,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    async fn replay_from(
        &self,
        mut root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );
            let transformed = self
                .upcaster
                .apply(morsel)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            let event = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            root.replay(env.version(), &event)?;
        }

        Ok(root)
    }
}
```

Then refactor `Repository::load` to delegate:

```rust
async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
    let root = AggregateRoot::<A>::new(id);
    self.replay_from(root, Version::INITIAL).await
}
```

**Step 4: Implement for ZeroCopyEventStore**

Same pattern using `BorrowingCodec`. Refactor its `load()` to delegate to `replay_from` as well.

**Step 5: Update store/mod.rs**

Add `mod replay;` — no public re-export (it's `pub(crate)`).

**Step 6: Run all existing tests — expected PASS (no behavior change)**

Run: `cargo test -p nexus-store --features json`

**Step 7: Commit**

```
refactor(store): extract ReplayFrom trait for shared replay logic
```

---

### Task 7: Create InMemorySnapshotStore for testing

**Files:**
- Create: `crates/nexus-store/src/snapshot/testing.rs`
- Modify: `crates/nexus-store/src/snapshot/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Add to `snapshot_tests.rs`:

```rust
#[cfg(feature = "testing")]
mod in_memory_tests {
    use super::*;
    use nexus_store::snapshot::InMemorySnapshotStore;

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let store = InMemorySnapshotStore::new();
        let result = store.load_snapshot(&String::from("agg-1")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = InMemorySnapshotStore::new();
        let id = String::from("agg-1");
        let version = Version::new(10).unwrap();
        let snap = PendingSnapshot::new(version, 1, vec![1, 2, 3]);

        store.save_snapshot(&id, &snap).await.unwrap();
        let loaded = store.load_snapshot(&id).await.unwrap().unwrap();

        assert_eq!(loaded.version(), version);
        assert_eq!(loaded.schema_version(), 1);
        assert_eq!(loaded.payload(), &[1, 2, 3]);
    }

    #[tokio::test]
    async fn save_overwrites_previous_snapshot() {
        let store = InMemorySnapshotStore::new();
        let id = String::from("agg-1");

        let snap1 = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![1]);
        store.save_snapshot(&id, &snap1).await.unwrap();

        let snap2 = PendingSnapshot::new(Version::new(20).unwrap(), 1, vec![2]);
        store.save_snapshot(&id, &snap2).await.unwrap();

        let loaded = store.load_snapshot(&id).await.unwrap().unwrap();
        assert_eq!(loaded.version(), Version::new(20).unwrap());
        assert_eq!(loaded.payload(), &[2]);
    }

    #[tokio::test]
    async fn different_aggregates_have_separate_snapshots() {
        let store = InMemorySnapshotStore::new();

        let snap1 = PendingSnapshot::new(Version::new(5).unwrap(), 1, vec![1]);
        store.save_snapshot(&String::from("agg-1"), &snap1).await.unwrap();

        let snap2 = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![2]);
        store.save_snapshot(&String::from("agg-2"), &snap2).await.unwrap();

        let loaded1 = store.load_snapshot(&String::from("agg-1")).await.unwrap().unwrap();
        let loaded2 = store.load_snapshot(&String::from("agg-2")).await.unwrap().unwrap();
        assert_eq!(loaded1.version(), Version::new(5).unwrap());
        assert_eq!(loaded2.version(), Version::new(10).unwrap());
    }

    #[tokio::test]
    async fn delete_removes_snapshot() {
        let store = InMemorySnapshotStore::new();
        let id = String::from("agg-1");

        let snap = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![1]);
        store.save_snapshot(&id, &snap).await.unwrap();

        store.delete_snapshot(&id).await.unwrap();
        let loaded = store.load_snapshot(&id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let store = InMemorySnapshotStore::new();
        let result = store.delete_snapshot(&String::from("nope")).await;
        assert!(result.is_ok());
    }
}
```

**Step 2: Run test — expected FAIL**

**Step 3: Implement InMemorySnapshotStore**

Create `crates/nexus-store/src/snapshot/testing.rs`:

```rust
use std::collections::HashMap;
use std::convert::Infallible;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingSnapshot;
use super::persisted::PersistedSnapshot;
use super::store::SnapshotStore;

/// In-memory snapshot store for tests.
///
/// Stores snapshots in a `HashMap` protected by a `RwLock`.
#[derive(Debug, Default)]
pub struct InMemorySnapshotStore {
    snapshots: RwLock<HashMap<String, StoredSnapshot>>,
}

#[derive(Debug, Clone)]
struct StoredSnapshot {
    version: nexus::Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl InMemorySnapshotStore {
    /// Create a new empty in-memory snapshot store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SnapshotStore for InMemorySnapshotStore {
    type Error = Infallible;

    async fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> Result<Option<PersistedSnapshot>, Infallible> {
        let snapshots = self.snapshots.read().await;
        let key = id.to_string();
        Ok(snapshots.get(&key).map(|s| {
            PersistedSnapshot::new(s.version, s.schema_version, s.payload.clone())
        }))
    }

    async fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> Result<(), Infallible> {
        let mut snapshots = self.snapshots.write().await;
        let key = id.to_string();
        snapshots.insert(
            key,
            StoredSnapshot {
                version: snapshot.version(),
                schema_version: snapshot.schema_version(),
                payload: snapshot.payload().to_vec(),
            },
        );
        Ok(())
    }

    async fn delete_snapshot(
        &self,
        id: &impl Id,
    ) -> Result<(), Infallible> {
        let mut snapshots = self.snapshots.write().await;
        snapshots.remove(&id.to_string());
        Ok(())
    }
}
```

Feature-gate in `snapshot/mod.rs`:

```rust
#[cfg(feature = "testing")]
mod testing;
#[cfg(feature = "testing")]
pub use testing::InMemorySnapshotStore;
```

**Step 4: Run tests — expected PASS**

Run: `cargo test -p nexus-store --features "snapshot,testing" -- snapshot_tests`

**Step 5: Commit**

```
feat(store): add InMemorySnapshotStore for snapshot testing
```

---

### Task 8: Create Snapshotting facade

**Files:**
- Create: `crates/nexus-store/src/store/snapshotting.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/snapshot_integration_tests.rs`

This is the core task — the `Snapshotting` decorator that wraps a `Repository` and adds snapshot load/save logic. The trigger is a type parameter (monomorphized, zero-cost) instead of `Box<dyn>`.

**Step 1: Write integration tests**

Create `crates/nexus-store/tests/snapshot_integration_tests.rs`. Define or reuse test aggregate types. Key test cases:

```rust
#![cfg(all(feature = "snapshot", feature = "json", feature = "testing"))]

// (use existing test aggregate types or define minimal ones)

#[tokio::test]
async fn load_without_snapshot_does_full_replay() {
    // Setup: EventStore with InMemoryStore, save 5 events
    // Wrap with Snapshotting + empty InMemorySnapshotStore
    // Load: should replay all 5 events, aggregate at version 5
}

#[tokio::test]
async fn save_triggers_snapshot_at_threshold() {
    // Setup: Snapshotting with EveryNEvents(3)
    // Save 3 events → snapshot should be stored
    // Verify snapshot exists in InMemorySnapshotStore
}

#[tokio::test]
async fn save_triggers_snapshot_on_boundary_crossing_batch() {
    // Setup: Snapshotting with EveryNEvents(5)
    // Save 3 events (v1-3), then 3 more (v4-6, crossing 5-boundary)
    // Verify snapshot taken at v6
}

#[tokio::test]
async fn load_from_snapshot_skips_replayed_events() {
    // Setup: Save 10 events, manually insert snapshot at version 5
    // Load: should restore from snapshot + replay events 6-10
    // Verify aggregate state matches full replay
}

#[tokio::test]
async fn schema_version_mismatch_falls_back_to_full_replay() {
    // Setup: Save events, create snapshot with schema_version=1
    // Create Snapshotting with schema_version=2
    // Load: should ignore snapshot, do full replay
}

#[tokio::test]
async fn snapshot_save_failure_does_not_fail_event_save() {
    // Setup: Use a SnapshotStore that returns Err on save
    // Save: should succeed (events persisted), snapshot failure ignored
}

#[tokio::test]
async fn after_event_types_trigger_snapshots_on_domain_milestone() {
    // Setup: Snapshotting with AfterEventTypes(["OrderCompleted"])
    // Save ItemAdded events → no snapshot
    // Save OrderCompleted → snapshot taken
}

#[tokio::test]
async fn lazy_snapshot_on_read_after_full_replay() {
    // Setup: Snapshotting with EveryNEvents(5) + on_read=true
    // Save 10 events without any snapshot
    // Load: full replay → snapshot created as side effect
    // Load again: snapshot hit + partial replay
}
```

**Step 2: Run tests — expected FAIL**

**Step 3: Implement Snapshotting**

Create `crates/nexus-store/src/store/snapshotting.rs`:

```rust
use crate::codec::Codec;
use crate::error::StoreError;
use crate::snapshot::{PendingSnapshot, PersistedSnapshot, SnapshotStore, SnapshotTrigger};
use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

use super::replay::ReplayFrom;
use super::repository::Repository;

/// Snapshot-aware repository decorator.
///
/// Wraps an inner repository (e.g., `EventStore` or `ZeroCopyEventStore`)
/// and adds transparent snapshot support:
///
/// - **Load:** tries the snapshot store first; on hit with matching schema
///   version, restores state from snapshot and replays only subsequent events.
///   On miss or schema mismatch, falls back to full event replay via the
///   inner repository. Optionally creates a snapshot after a full replay
///   (lazy/on-read snapshotting) if `snapshot_on_read` is enabled.
///
/// - **Save:** delegates event persistence to the inner repository, then
///   checks the trigger to optionally persist a snapshot of the current state.
///   Snapshot save is best-effort — failures are silently ignored.
///
/// The trigger type `T` is a generic parameter (not `Box<dyn>`) for
/// zero-cost monomorphization — the compiler inlines `should_snapshot()`
/// calls entirely.
pub struct Snapshotting<R, SS, SC, T> {
    inner: R,
    snapshot_store: SS,
    snapshot_codec: SC,
    trigger: T,
    schema_version: u32,
    snapshot_on_read: bool,
}

impl<R, SS, SC, T> Snapshotting<R, SS, SC, T> {
    /// Create a new snapshot-aware repository.
    pub(crate) fn new(
        inner: R,
        snapshot_store: SS,
        snapshot_codec: SC,
        trigger: T,
        schema_version: u32,
        snapshot_on_read: bool,
    ) -> Self {
        Self {
            inner,
            snapshot_store,
            snapshot_codec,
            trigger,
            schema_version,
            snapshot_on_read,
        }
    }
}

impl<A, R, SS, SC, T> Repository<A> for Snapshotting<R, SS, SC, T>
where
    A: Aggregate,
    R: Repository<A, Error = StoreError> + ReplayFrom<A>,
    SS: SnapshotStore,
    SC: Codec<A::State>,
    T: SnapshotTrigger,
    EventOf<A>: DomainEvent,
{
    type Error = StoreError;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        // Try snapshot first.
        let snapshot_hit = self.try_load_from_snapshot(&id).await;

        if let Some((root, from)) = snapshot_hit {
            // Snapshot hit — partial replay from snapshot version.
            return self.inner.replay_from(root, from).await;
        }

        // Fallback: full replay.
        let root = self.inner.load(id.clone()).await?;

        // Lazy snapshot: if enabled and aggregate has events, snapshot now.
        if self.snapshot_on_read {
            if let Some(version) = root.version() {
                self.try_save_snapshot(&root, version).await;
            }
        }

        Ok(root)
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), StoreError> {
        let old_version = aggregate.version();

        // Delegate event persistence to inner.
        self.inner.save(aggregate, events).await?;

        // Check trigger — maybe snapshot.
        if !events.is_empty() {
            if let Some(new_version) = aggregate.version() {
                let event_names: Vec<&str> = events.iter().map(|e| e.name()).collect();
                if self.trigger.should_snapshot(old_version, new_version, &event_names) {
                    self.try_save_snapshot(aggregate, new_version).await;
                }
            }
        }

        Ok(())
    }
}

impl<R, SS, SC, T> Snapshotting<R, SS, SC, T>
where
    SS: SnapshotStore,
{
    /// Try to load a snapshot. Returns (root, next_version_to_replay) on hit.
    /// Returns None on miss, schema mismatch, or any error (best-effort).
    async fn try_load_from_snapshot<A>(
        &self,
        id: &A::Id,
    ) -> Option<(AggregateRoot<A>, Version)>
    where
        A: Aggregate,
        SC: Codec<A::State>,
    {
        let holder = self.snapshot_store.load_snapshot(id).await.ok()??;

        if holder.schema_version() != self.schema_version {
            return None;
        }

        let state = self
            .snapshot_codec
            .decode("", holder.payload())
            .ok()?;

        let version = holder.version();
        let root = AggregateRoot::<A>::restore(id.clone(), state, version);
        let next = version.next()?;
        Some((root, next))
    }

    /// Best-effort snapshot save. Errors are silently ignored.
    async fn try_save_snapshot<A>(
        &self,
        aggregate: &AggregateRoot<A>,
        version: Version,
    )
    where
        A: Aggregate,
        SC: Codec<A::State>,
    {
        if let Ok(payload) = self.snapshot_codec.encode(aggregate.state()) {
            let snap = PendingSnapshot::new(version, self.schema_version, payload);
            let _ = self.snapshot_store.save_snapshot(aggregate.id(), &snap).await;
        }
    }
}
```

Update `store/mod.rs`:

```rust
#[cfg(feature = "snapshot")]
mod snapshotting;

// existing exports...

#[cfg(feature = "snapshot")]
pub use snapshotting::Snapshotting;
```

Update `lib.rs` to re-export `Snapshotting`.

**Step 4: Run tests**

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- snapshot_integration`
Expected: PASS

**Step 5: Commit**

```
feat(store): add Snapshotting repository decorator with lazy on-read support
```

---

### Task 9: Builder integration

**Files:**
- Modify: `crates/nexus-store/src/store/builder.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs` (add builder tests)

**Step 1: Write builder tests**

Add to `snapshot_tests.rs` a `builder_tests` module that verifies the builder API compiles and produces working repositories. Test both `build()` and `build_zero_copy()` with snapshot config.

**Step 2: Implement builder changes**

Add typestate marker and snapshot config to `builder.rs`:

```rust
/// Marker: no snapshot configured (default).
pub struct NoSnapshot;

/// Snapshot configuration, created by builder methods.
#[cfg(feature = "snapshot")]
pub struct WithSnapshot<SS, SC, T> {
    store: SS,
    codec: SC,
    trigger: T,
    schema_version: u32,
    snapshot_on_read: bool,
}
```

Extend `RepositoryBuilder` with a `Snap` type param defaulting to `NoSnapshot`:

```rust
pub struct RepositoryBuilder<S, C, U, Snap = NoSnapshot> {
    store: Store<S>,
    codec: C,
    upcaster: U,
    snapshot: Snap,
}
```

Existing `.codec()` and `.upcaster()` methods carry `Snap` through. Existing `.build()` for `NoSnapshot` returns `EventStore` unchanged.

Add snapshot builder methods (feature-gated):

```rust
#[cfg(feature = "snapshot")]
impl<S, C, U> RepositoryBuilder<S, C, U, NoSnapshot> {
    /// Configure a snapshot store, transitioning the builder to snapshot mode.
    pub fn snapshot_store<SS: SnapshotStore>(
        self,
        snapshot_store: SS,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, /* codec */, EveryNEvents>> {
        // Default trigger: EveryNEvents(100)
        // Default schema_version: 1
        // Default snapshot_on_read: false
        // Snapshot codec: NeedsSnapshotCodec or JsonCodec (with snapshot-json feature)
    }
}
```

Add `.snapshot_codec()`, `.snapshot_trigger()`, `.snapshot_schema_version()`, `.snapshot_on_read()` methods on the `WithSnapshot` state.

Add `.build()` for `WithSnapshot`:

```rust
#[cfg(feature = "snapshot")]
impl<S, C, U, SS, SC, T> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, T>>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
    SC: Send + Sync + 'static,
{
    #[must_use]
    pub fn build(self) -> Snapshotting<EventStore<S, C, U>, SS, SC, T> {
        let inner = EventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(inner, snap.store, snap.codec, snap.trigger, snap.schema_version, snap.snapshot_on_read)
    }

    #[must_use]
    pub fn build_zero_copy(self) -> Snapshotting<ZeroCopyEventStore<S, C, U>, SS, SC, T> {
        let inner = ZeroCopyEventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(inner, snap.store, snap.codec, snap.trigger, snap.schema_version, snap.snapshot_on_read)
    }
}
```

**Usage:**

```rust
// No snapshots — unchanged
let repo = store.repository().build();

// With snapshots (snapshot-json feature auto-wires JsonCodec)
let repo = store.repository()
    .snapshot_store(fjall_snapshots)
    .snapshot_trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
    .build();

// Custom snapshot codec + semantic trigger + lazy on-read
let repo = store.repository()
    .snapshot_store(fjall_snapshots)
    .snapshot_codec(RkyvCodec)
    .snapshot_trigger(AfterEventTypes::new(&["OrderCompleted"]))
    .snapshot_on_read(true)
    .build();
```

**Step 3: Run tests**

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- builder_tests`

**Step 4: Commit**

```
feat(store): integrate snapshot config into RepositoryBuilder
```

---

### Task 10: Full integration test suite

**Files:**
- Modify: `crates/nexus-store/tests/snapshot_integration_tests.rs`

Write comprehensive tests covering the 4 mandatory test categories from CLAUDE.md:

**1. Sequence/Protocol Tests:**
- Save N events → snapshot triggers → load → partial replay → save more → snapshot again
- Save below threshold → no snapshot → save more → crosses threshold → snapshot
- Multiple save/load cycles with snapshot invalidation in between
- Batch saves that cross boundaries at non-multiples (the audit-discovered bug scenario)

**2. Lifecycle Tests:**
- Create aggregate → save → snapshot → load from snapshot → verify state
- Load from snapshot → save more events → load again → verify new snapshot version used
- Lazy on-read: full replay creates snapshot → subsequent load uses it

**3. Defensive Boundary Tests:**
- Empty event slice save (no-op, no snapshot trigger)
- Schema version 0 rejection in PendingSnapshot
- Snapshot at Version u64::MAX (no next version — edge case)
- Snapshot codec returns error → fallback to full replay
- Snapshot store returns error on load → fallback to full replay
- Snapshot store returns error on save → event save still succeeds
- AfterEventTypes with empty event names → no trigger

**4. Linearizability/Isolation Tests:**
- Concurrent loads from same snapshot → each gets independent copy
- Save while another load is reading → no corruption (if applicable with InMemoryStore)

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- snapshot_integration`

**Commit:**

```
test(store): comprehensive snapshot integration tests
```

---

### Task 11: Fjall snapshot adapter

**Files:**
- Modify: `crates/nexus-fjall/Cargo.toml`
- Create: `crates/nexus-fjall/src/snapshot_store.rs`
- Create: `crates/nexus-fjall/src/snapshot_encoding.rs`
- Modify: `crates/nexus-fjall/src/store.rs`
- Modify: `crates/nexus-fjall/src/builder.rs`
- Modify: `crates/nexus-fjall/src/lib.rs`
- Test: `crates/nexus-fjall/tests/snapshot_tests.rs`

**Step 1: Add snapshot feature to nexus-fjall**

In `crates/nexus-fjall/Cargo.toml`:

```toml
[features]
snapshot = ["nexus-store/snapshot"]
```

**Step 2: Design the snapshot partition**

Third partition in fjall, point-read optimized (like `streams`, not scan-optimized like `events`).

Key: numeric stream ID (`u64 BE`) — reuse the same numeric ID mapping from the `streams` partition.
Value: `[u32 LE schema_version][u64 BE version][payload]`

This means snapshot load/save needs access to the numeric stream ID, which requires the `streams` partition for lookup.

**Step 3: Implement snapshot encoding**

Create `crates/nexus-fjall/src/snapshot_encoding.rs`:

```rust
pub(crate) const SNAPSHOT_KEY_SIZE: usize = 8;  // u64 BE stream numeric ID
pub(crate) const SNAPSHOT_VALUE_HEADER_SIZE: usize = 12;  // u32 LE schema + u64 BE version

pub(crate) fn encode_snapshot_key(stream_numeric_id: u64) -> [u8; SNAPSHOT_KEY_SIZE] {
    stream_numeric_id.to_be_bytes()
}

pub(crate) fn encode_snapshot_value(
    buf: &mut Vec<u8>,
    schema_version: u32,
    version: u64,
    payload: &[u8],
) {
    buf.clear();
    buf.reserve(SNAPSHOT_VALUE_HEADER_SIZE + payload.len());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&version.to_be_bytes());
    buf.extend_from_slice(payload);
}

pub(crate) fn decode_snapshot_value(
    value: &[u8],
) -> Result<(u32, u64, &[u8]), DecodeError> {
    if value.len() < SNAPSHOT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort { min: SNAPSHOT_VALUE_HEADER_SIZE, actual: value.len() });
    }
    let schema_version = u32::from_le_bytes(value[0..4].try_into().unwrap());
    let version = u64::from_be_bytes(value[4..12].try_into().unwrap());
    let payload = &value[12..];
    Ok((schema_version, version, payload))
}
```

**Step 4: Add snapshot partition to FjallStore**

Add a `snapshots: Option<TxPartitionHandle>` field to `FjallStore` (Option so it's None when snapshot feature is off, or when not configured).

Extend `FjallStoreBuilder` with `.snapshots_config()` method and create the partition in `.open()`.

**Step 5: Implement SnapshotStore for FjallStore**

The implementation needs to:
1. Look up numeric stream ID from the `streams` partition
2. Read/write the `snapshots` partition using that numeric ID as key
3. `delete_snapshot`: remove the key from the `snapshots` partition

Implement behind `#[cfg(feature = "snapshot")]`.

**Step 6: Write tests**

Key tests (4 categories):
- **Sequence:** Save snapshot → load → overwrite → load latest
- **Lifecycle:** Save → close → reopen → snapshot persisted
- **Defensive:** Load from nonexistent stream → None; corrupt snapshot value → error
- **Isolation:** Concurrent snapshot reads don't interfere

**Step 7: Commit**

```
feat(fjall): add FjallSnapshotStore with dedicated snapshot partition
```

---

### Task 12: Update re-exports and run full CI suite

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` (ensure all snapshot types re-exported)
- Modify: `crates/nexus-store/src/snapshot/mod.rs`

**Step 1: Verify all public types are re-exported**

Ensure `lib.rs` has:

```rust
#[cfg(feature = "snapshot")]
pub use snapshot::{
    AfterEventTypes, EveryNEvents, PendingSnapshot, PersistedSnapshot,
    SnapshotStore, SnapshotTrigger,
};
#[cfg(all(feature = "snapshot", feature = "testing"))]
pub use snapshot::InMemorySnapshotStore;
```

**Step 2: Run full test suite**

```bash
# Without snapshot feature — nothing breaks
cargo test --all

# With snapshot feature
cargo test --all --features nexus-store/snapshot,nexus-store/testing

# With snapshot-json
cargo test --all --features nexus-store/snapshot-json,nexus-store/testing

# Clippy
cargo clippy --all-targets -- --deny warnings
cargo clippy --all-targets --features nexus-store/snapshot -- --deny warnings

# Format
cargo fmt --all --check
taplo fmt --check
```

**Step 3: Update workspace-hack if needed**

```bash
cargo hakari generate
cargo hakari manage-deps
cargo hakari verify
```

**Step 4: Commit**

```
chore(store): finalize snapshot re-exports and verify CI
```

---

### Task 13: Update design doc with performance guidance

**Files:**
- Modify: `docs/plans/2026-04-10-snapshot-design.md`

Add a "When to use snapshots" section with benchmark guidance:

| Event Count | Snapshot Value |
|-------------|---------------|
| < 50 | Always hurts — overhead exceeds benefit |
| 50–200 | Rarely helps unless deserialization is expensive |
| 200–500 | Measure. Depends on event size and apply complexity |
| 500+ | Likely helps |
| 10,000+ | Essential — but also consider "Closing the Books" pattern (see #139) |

Document the preferred alternative: short-lived streams with natural lifecycle boundaries eliminate the need for snapshots entirely. Reference issue #139.

**Commit:**

```
docs: add snapshot performance guidance and alternatives
```
