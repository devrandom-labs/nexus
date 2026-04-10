# Aggregate Snapshot Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add feature-gated aggregate snapshot support to nexus-store so rehydration skips replaying the full event stream.

**Architecture:** `Snapshotting<R, SS, SC>` decorator wraps `EventStore`/`ZeroCopyEventStore`, intercepts `load()`/`save()` to check/write snapshots. Extract shared replay logic into a `pub(crate)` trait `ReplayFrom<A>` to avoid duplicating the stream replay loop. All snapshot types live behind `#[cfg(feature = "snapshot")]`. `()` serves as no-op `SnapshotStore`. Builder typestate gains a `Snap` param (`NoSnapshot` or `WithSnapshot<SS, SC>`) to produce either plain `EventStore` or `Snapshotting<EventStore, SS, SC>`.

**Tech Stack:** Rust, nexus kernel (`AggregateRoot`), nexus-store traits, fjall LSM storage.

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

### Task 2: Create PendingSnapshot struct

**Files:**
- Create: `crates/nexus-store/src/snapshot/pending.rs`
- Create: `crates/nexus-store/src/snapshot/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Create `crates/nexus-store/tests/snapshot_tests.rs`:

```rust
#![cfg(feature = "snapshot")]

use nexus_store::snapshot::{PendingSnapshot};
use nexus::Version;

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
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store --features snapshot -- snapshot_tests`
Expected: FAIL — module `snapshot` doesn't exist

**Step 3: Create the snapshot module**

Create `crates/nexus-store/src/snapshot/mod.rs`:

```rust
mod pending;

pub use pending::PendingSnapshot;
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
feat(store): add PendingSnapshot type for snapshot write path
```

---

### Task 3: Create SnapshotRead trait

**Files:**
- Create: `crates/nexus-store/src/snapshot/read.rs`
- Modify: `crates/nexus-store/src/snapshot/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Add to `snapshot_tests.rs`:

```rust
use nexus_store::snapshot::SnapshotRead;

struct TestHolder {
    version: Version,
    schema_version: u32,
    data: Vec<u8>,
}

impl SnapshotRead for TestHolder {
    fn version(&self) -> Version { self.version }
    fn schema_version(&self) -> u32 { self.schema_version }
    fn payload(&self) -> &[u8] { &self.data }
}

#[test]
fn snapshot_read_impl_provides_borrowed_access() {
    let holder = TestHolder {
        version: Version::new(10).unwrap(),
        schema_version: 2,
        data: vec![4, 5, 6],
    };
    assert_eq!(holder.version(), Version::new(10).unwrap());
    assert_eq!(holder.schema_version(), 2);
    assert_eq!(holder.payload(), &[4, 5, 6]);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store --features snapshot -- snapshot_read`
Expected: FAIL — `SnapshotRead` doesn't exist

**Step 3: Create SnapshotRead trait**

Create `crates/nexus-store/src/snapshot/read.rs`:

```rust
use nexus::Version;

/// Borrowed access to a persisted snapshot (read path).
///
/// Implemented by adapter-specific holder types. The holder owns
/// the raw bytes read from storage; `payload()` borrows from it,
/// enabling zero-copy decode via [`BorrowingCodec`](crate::BorrowingCodec).
pub trait SnapshotRead {
    /// The aggregate version at the time of the snapshot.
    fn version(&self) -> Version;

    /// The schema version for invalidation checking.
    fn schema_version(&self) -> u32;

    /// The serialized state bytes, borrowed from the holder.
    fn payload(&self) -> &[u8];
}
```

Update `snapshot/mod.rs`:

```rust
mod pending;
mod read;

pub use pending::PendingSnapshot;
pub use read::SnapshotRead;
```

**Step 4: Run test**

Run: `cargo test -p nexus-store --features snapshot -- snapshot_read`
Expected: PASS

**Step 5: Commit**

```
feat(store): add SnapshotRead trait for borrowed snapshot access
```

---

### Task 4: Create SnapshotStore trait + () no-op

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
```

**Step 2: Run test — expected FAIL**

**Step 3: Create SnapshotStore trait**

Create `crates/nexus-store/src/snapshot/store.rs`:

```rust
use std::convert::Infallible;
use std::future::Future;

use nexus::Id;

use super::pending::PendingSnapshot;
use super::read::SnapshotRead;

/// Byte-level snapshot storage trait.
///
/// Adapters (fjall, postgres, etc.) implement this to persist and
/// retrieve serialized aggregate state. The GAT `Holder<'a>` enables
/// zero-copy reads — the holder owns the raw bytes and
/// [`SnapshotRead::payload()`] borrows from it.
///
/// `()` is the no-op implementation: `load_snapshot` always returns
/// `None`, `save_snapshot` silently discards. Used when snapshot
/// support is not configured.
pub trait SnapshotStore: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Type that owns snapshot bytes on the read path.
    type Holder<'a>: SnapshotRead + 'a
    where
        Self: 'a;

    /// Load the most recent snapshot for the given aggregate.
    ///
    /// Returns `None` if no snapshot exists.
    fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<Option<Self::Holder<'_>>, Self::Error>> + Send;

    /// Persist a snapshot for the given aggregate.
    ///
    /// Overwrites any existing snapshot for this aggregate.
    fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — snapshots disabled
// ═══════════════════════════════════════════════════════════════════════════

/// Uninhabited holder for the `()` no-op snapshot store.
///
/// This type can never be constructed, matching the fact that
/// the no-op store always returns `None` from `load_snapshot`.
pub enum NeverSnapshot {}

impl SnapshotRead for NeverSnapshot {
    fn version(&self) -> nexus::Version {
        match *self {}
    }
    fn schema_version(&self) -> u32 {
        match *self {}
    }
    fn payload(&self) -> &[u8] {
        match *self {}
    }
}

impl SnapshotStore for () {
    type Error = Infallible;
    type Holder<'a> = NeverSnapshot;

    async fn load_snapshot(
        &self,
        _id: &impl Id,
    ) -> Result<Option<NeverSnapshot>, Infallible> {
        Ok(None)
    }

    async fn save_snapshot(
        &self,
        _id: &impl Id,
        _snapshot: &PendingSnapshot,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}
```

Update `snapshot/mod.rs`:

```rust
mod pending;
mod read;
mod store;

pub use pending::PendingSnapshot;
pub use read::SnapshotRead;
pub use store::{NeverSnapshot, SnapshotStore};
```

**Step 4: Run tests — expected PASS**

**Step 5: Commit**

```
feat(store): add SnapshotStore trait with () no-op implementation
```

---

### Task 5: Create SnapshotTrigger trait + EveryNEvents

**Files:**
- Create: `crates/nexus-store/src/snapshot/trigger.rs`
- Modify: `crates/nexus-store/src/snapshot/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs`

**Step 1: Write the failing test**

Add to `snapshot_tests.rs`:

```rust
use std::num::NonZeroU64;
use nexus_store::snapshot::{SnapshotTrigger, EveryNEvents};

#[test]
fn every_n_events_triggers_at_multiples() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());
    assert!(!trigger.should_snapshot(Version::new(1).unwrap()));
    assert!(!trigger.should_snapshot(Version::new(99).unwrap()));
    assert!(trigger.should_snapshot(Version::new(100).unwrap()));
    assert!(!trigger.should_snapshot(Version::new(101).unwrap()));
    assert!(trigger.should_snapshot(Version::new(200).unwrap()));
}

#[test]
fn every_1_event_always_triggers() {
    let trigger = EveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_snapshot(Version::new(1).unwrap()));
    assert!(trigger.should_snapshot(Version::new(2).unwrap()));
    assert!(trigger.should_snapshot(Version::new(999).unwrap()));
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
/// Checked after each successful `save()`. The trigger receives the
/// aggregate's new version and returns whether a snapshot should be taken.
pub trait SnapshotTrigger: Send + Sync {
    /// Whether a snapshot should be taken at this version.
    fn should_snapshot(&self, new_version: Version) -> bool;
}

/// Trigger a snapshot every N events (at version multiples of N).
///
/// For example, `EveryNEvents(100)` triggers at versions 100, 200, 300, etc.
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl SnapshotTrigger for EveryNEvents {
    fn should_snapshot(&self, new_version: Version) -> bool {
        new_version.as_u64() % self.0.get() == 0
    }
}
```

Update `snapshot/mod.rs` to export `SnapshotTrigger` and `EveryNEvents`.

**Step 4: Run tests — expected PASS**

**Step 5: Commit**

```
feat(store): add SnapshotTrigger trait and EveryNEvents strategy
```

---

### Task 6: Add AggregateRoot::restore to kernel

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

### Task 7: Extract ReplayFrom trait from EventStore/ZeroCopyEventStore

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

In `event_store.rs`, add:

```rust
use super::replay::ReplayFrom;
```

Then add the impl block:

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

Same pattern but using `BorrowingCodec`:

```rust
impl<A, S, C, U> ReplayFrom<A> for ZeroCopyEventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
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
            let event: &EventOf<A> = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            root.replay(env.version(), event)?;
        }

        Ok(root)
    }
}
```

Refactor `ZeroCopyEventStore::load` to delegate similarly.

**Step 5: Update store/mod.rs**

Add:

```rust
mod replay;
```

No public re-export — `replay` is `pub(crate)`.

**Step 6: Run all existing tests — expected PASS (no behavior change)**

Run: `cargo test -p nexus-store --features json`

**Step 7: Commit**

```
refactor(store): extract ReplayFrom trait for shared replay logic
```

---

### Task 8: Create InMemorySnapshotStore for testing

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
}
```

**Step 2: Run test — expected FAIL**

**Step 3: Implement InMemorySnapshotStore**

Create `crates/nexus-store/src/snapshot/testing.rs`:

```rust
use std::collections::HashMap;
use std::convert::Infallible;

use nexus::{Id, Version};
use tokio::sync::RwLock;

use super::pending::PendingSnapshot;
use super::read::SnapshotRead;
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
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

/// Holder type for borrowed snapshot access.
#[derive(Debug)]
pub struct InMemorySnapshotHolder {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl SnapshotRead for InMemorySnapshotHolder {
    fn version(&self) -> Version {
        self.version
    }
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn payload(&self) -> &[u8] {
        &self.payload
    }
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
    type Holder<'a> = InMemorySnapshotHolder;

    async fn load_snapshot(
        &self,
        id: &impl Id,
    ) -> Result<Option<InMemorySnapshotHolder>, Infallible> {
        let snapshots = self.snapshots.read().await;
        let key = id.to_string();
        Ok(snapshots.get(&key).map(|s| InMemorySnapshotHolder {
            version: s.version,
            schema_version: s.schema_version,
            payload: s.payload.clone(),
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
}
```

Feature-gate in `snapshot/mod.rs`:

```rust
#[cfg(feature = "testing")]
mod testing;
#[cfg(feature = "testing")]
pub use testing::{InMemorySnapshotStore, InMemorySnapshotHolder};
```

**Step 4: Run tests — expected PASS**

Run: `cargo test -p nexus-store --features "snapshot,testing" -- snapshot_tests`

**Step 5: Commit**

```
feat(store): add InMemorySnapshotStore for snapshot testing
```

---

### Task 9: Create Snapshotting facade

**Files:**
- Create: `crates/nexus-store/src/store/snapshotting.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`
- Test: `crates/nexus-store/tests/snapshot_integration_tests.rs`

This is the core task — the `Snapshotting` decorator that wraps a `Repository` and adds snapshot load/save logic.

**Step 1: Write integration tests**

Create `crates/nexus-store/tests/snapshot_integration_tests.rs`. These tests need a full test aggregate. Reference existing test aggregates from `event_store_tests.rs` or define test helpers.

Key test cases:

```rust
#![cfg(feature = "snapshot")]

// (Use existing test aggregate types or define minimal ones)

#[tokio::test]
async fn load_without_snapshot_does_full_replay() {
    // Setup: EventStore with InMemoryStore, save 5 events
    // Add Snapshotting with InMemorySnapshotStore (empty)
    // Load: should replay all 5 events, aggregate at version 5
}

#[tokio::test]
async fn save_triggers_snapshot_at_threshold() {
    // Setup: Snapshotting with EveryNEvents(3)
    // Save 3 events → snapshot should be stored
    // Verify snapshot exists in InMemorySnapshotStore
}

#[tokio::test]
async fn load_from_snapshot_skips_replayed_events() {
    // Setup: Save 10 events, snapshot at version 5
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
async fn snapshot_load_failure_falls_back_to_full_replay() {
    // Setup: Use a SnapshotStore that returns Err on load
    // Load: should fall back to full event replay, not propagate error
}

#[tokio::test]
async fn snapshot_save_failure_does_not_fail_event_save() {
    // Setup: Use a SnapshotStore that returns Err on save
    // Save: should succeed (events persisted), snapshot failure ignored
}
```

**Step 2: Run tests — expected FAIL**

**Step 3: Implement Snapshotting**

Create `crates/nexus-store/src/store/snapshotting.rs`:

```rust
use crate::codec::Codec;
use crate::error::StoreError;
use crate::snapshot::{PendingSnapshot, SnapshotStore, SnapshotTrigger};
use nexus::{Aggregate, AggregateRoot, EventOf, Version};

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
///   inner repository.
///
/// - **Save:** delegates event persistence to the inner repository, then
///   checks the trigger to optionally persist a snapshot of the current state.
///   Snapshot save is best-effort — failures are silently ignored.
pub struct Snapshotting<R, SS, SC> {
    inner: R,
    snapshot_store: SS,
    snapshot_codec: SC,
    trigger: Box<dyn SnapshotTrigger>,
    schema_version: u32,
}

impl<R, SS, SC> Snapshotting<R, SS, SC> {
    /// Create a new snapshot-aware repository.
    pub(crate) fn new(
        inner: R,
        snapshot_store: SS,
        snapshot_codec: SC,
        trigger: Box<dyn SnapshotTrigger>,
        schema_version: u32,
    ) -> Self {
        Self {
            inner,
            snapshot_store,
            snapshot_codec,
            trigger,
            schema_version,
        }
    }
}

impl<A, R, SS, SC> Repository<A> for Snapshotting<R, SS, SC>
where
    A: Aggregate,
    R: Repository<A, Error = StoreError> + ReplayFrom<A>,
    SS: SnapshotStore,
    SC: Codec<A::State>,
    EventOf<A>: nexus::DomainEvent,
{
    type Error = StoreError;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        // Try snapshot first.
        if let Ok(Some(holder)) = self
            .snapshot_store
            .load_snapshot(&id)
            .await
            .map_err(|_| ()) // discard error — snapshot is best-effort
        {
            if holder.schema_version() == self.schema_version {
                // Schema matches — decode state.
                if let Ok(state) = self
                    .snapshot_codec
                    .decode("", holder.payload())
                    .map_err(|_| ()) // discard decode error
                {
                    let root = AggregateRoot::<A>::restore(id, state, holder.version());
                    // Replay events after snapshot version.
                    if let Some(next) = holder.version().next() {
                        return self.inner.replay_from(root, next).await;
                    }
                    // Snapshot is at u64::MAX — no more events possible.
                    return Ok(root);
                }
            }
        }

        // Fallback: full replay.
        self.inner.load(id).await
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), StoreError> {
        // Delegate event persistence to inner.
        self.inner.save(aggregate, events).await?;

        // Check trigger — maybe snapshot.
        if let Some(version) = aggregate.version() {
            if self.trigger.should_snapshot(version) {
                // Best-effort: encode state and save snapshot.
                if let Ok(payload) = self.snapshot_codec.encode(aggregate.state()) {
                    let snap = PendingSnapshot::new(version, self.schema_version, payload);
                    // Ignore save errors — snapshot is an optimization.
                    let _ = self.snapshot_store.save_snapshot(aggregate.id(), &snap).await;
                }
            }
        }

        Ok(())
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

Update `lib.rs` to re-export:

```rust
#[cfg(feature = "snapshot")]
pub use store::Snapshotting;
```

**Step 4: Run tests**

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- snapshot_integration`
Expected: PASS

**Step 5: Commit**

```
feat(store): add Snapshotting repository decorator
```

---

### Task 10: Builder integration

**Files:**
- Modify: `crates/nexus-store/src/store/builder.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/snapshot_tests.rs` (add builder tests)

**Step 1: Write builder tests**

Add to `snapshot_tests.rs`:

```rust
#[cfg(all(feature = "snapshot", feature = "testing", feature = "json"))]
mod builder_tests {
    use std::num::NonZeroU64;
    use nexus_store::{Store, Repository};
    use nexus_store::snapshot::{EveryNEvents, InMemorySnapshotStore};
    use nexus_store::testing::InMemoryStore;

    #[tokio::test]
    async fn builder_produces_snapshotting_repository() {
        let raw = InMemoryStore::default();
        let store = Store::new(raw);
        let snap_store = InMemorySnapshotStore::new();

        let repo = store
            .repository()
            .snapshot_store(snap_store)
            .snapshot_trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
            .snapshot_schema_version(1)
            .build();

        // Verify it compiles and implements Repository — load an empty aggregate
        // (use existing test aggregate type)
    }

    #[tokio::test]
    async fn builder_without_snapshot_produces_plain_event_store() {
        let raw = InMemoryStore::default();
        let store = Store::new(raw);

        let repo = store.repository().build();
        // This should still work exactly as before
    }
}
```

**Step 2: Implement builder changes**

Add typestate marker and snapshot config to `builder.rs`:

```rust
#[cfg(feature = "snapshot")]
use crate::snapshot::{SnapshotStore, SnapshotTrigger, EveryNEvents};
#[cfg(feature = "snapshot")]
use super::snapshotting::Snapshotting;

/// Marker: no snapshot configured (default).
pub struct NoSnapshot;

/// Snapshot configuration.
#[cfg(feature = "snapshot")]
pub struct WithSnapshot<SS, SC> {
    store: SS,
    codec: SC,
    trigger: Box<dyn SnapshotTrigger>,
    schema_version: u32,
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

Add snapshot builder methods (feature-gated):

```rust
#[cfg(feature = "snapshot")]
impl<S, C, U> RepositoryBuilder<S, C, U, NoSnapshot> {
    /// Configure a snapshot store, transitioning the builder to snapshot mode.
    ///
    /// After calling this, `.build()` will produce a `Snapshotting` repository
    /// instead of a plain `EventStore`.
    #[must_use]
    pub fn snapshot_store<SS: SnapshotStore>(
        self,
        snapshot_store: SS,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, NeedsSnapshotCodec>> {
        // NeedsSnapshotCodec is !Send, like NeedsCodec
        // ... (or default to JsonCodec with snapshot-json feature)
    }
}
```

Handle snapshot codec defaulting (with `snapshot-json` feature):

```rust
#[cfg(all(feature = "snapshot-json"))]
impl<S, C, U, SS> RepositoryBuilder<S, C, U, WithSnapshot<SS, NeedsSnapshotCodec>> {
    // Auto-fill JsonCodec as default snapshot codec
}
```

Add `.build()` for `WithSnapshot`:

```rust
#[cfg(feature = "snapshot")]
impl<S, C, U, SS, SC> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC>>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
    SC: Send + Sync + 'static,
{
    #[must_use]
    pub fn build(self) -> Snapshotting<EventStore<S, C, U>, SS, SC> {
        let inner = EventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(inner, snap.store, snap.codec, snap.trigger, snap.schema_version)
    }

    #[must_use]
    pub fn build_zero_copy(self) -> Snapshotting<ZeroCopyEventStore<S, C, U>, SS, SC> {
        let inner = ZeroCopyEventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(inner, snap.store, snap.codec, snap.trigger, snap.schema_version)
    }
}
```

**Note:** The existing `.build()` for `NoSnapshot` returns `EventStore` — unchanged. The new `.build()` for `WithSnapshot` returns `Snapshotting<EventStore, ...>`. Both implement `Repository<A>`, so callers don't care which one they get.

Exact method signatures, `NeedsSnapshotCodec` guard, and `snapshot-json` defaulting should follow the same pattern as the existing `NeedsCodec` / `json` feature for event codecs.

**Step 3: Update module exports**

In `store/mod.rs`, add:

```rust
pub use builder::NoSnapshot;
#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
```

**Step 4: Run tests**

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- builder_tests`

**Step 5: Commit**

```
feat(store): integrate snapshot config into RepositoryBuilder
```

---

### Task 11: Full integration test suite

**Files:**
- Modify: `crates/nexus-store/tests/snapshot_integration_tests.rs`

Write comprehensive tests covering the 4 mandatory test categories from CLAUDE.md:

**1. Sequence/Protocol Tests:**
- Save N events → snapshot triggers → load → partial replay → save more → snapshot again
- Save below threshold → no snapshot → save more → crosses threshold → snapshot
- Multiple save/load cycles with snapshot invalidation in between

**2. Lifecycle Tests:**
- Create aggregate → save → snapshot → load from snapshot → verify state
- Load from snapshot → save more events → load again → verify new snapshot version used

**3. Defensive Boundary Tests:**
- Empty event slice save (no-op, no snapshot trigger)
- Schema version 0 rejection in PendingSnapshot
- Snapshot at Version u64::MAX (no next version — edge case)
- Snapshot codec returns error → fallback to full replay
- Snapshot store returns error on load → fallback to full replay
- Snapshot store returns error on save → event save still succeeds

**4. Linearizability/Isolation Tests:**
- Concurrent loads from same snapshot → each gets independent copy
- Save while another load is reading → no corruption (if applicable with InMemoryStore)

Run: `cargo test -p nexus-store --features "snapshot,json,testing" -- snapshot_integration`

**Commit:**

```
test(store): comprehensive snapshot integration tests
```

---

### Task 12: Fjall snapshot adapter

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
        return Err(DecodeError::ValueTooShort { ... });
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

Implement behind `#[cfg(feature = "snapshot")]`.

**Step 6: Write tests**

Key tests:
- Save snapshot → load snapshot roundtrip
- Overwrite snapshot → latest returned
- Load from nonexistent stream → None
- Corrupt snapshot value → error
- Close and reopen → snapshot persisted

**Step 7: Commit**

```
feat(fjall): add FjallSnapshotStore with dedicated snapshot partition
```

---

### Task 13: Update re-exports and run full CI suite

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` (ensure all snapshot types re-exported)
- Modify: `crates/nexus-store/src/snapshot/mod.rs`

**Step 1: Verify all public types are re-exported**

Ensure `lib.rs` has:

```rust
#[cfg(feature = "snapshot")]
pub use snapshot::{
    EveryNEvents, InMemorySnapshotStore, NeverSnapshot,
    PendingSnapshot, SnapshotRead, SnapshotStore, SnapshotTrigger,
};
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
