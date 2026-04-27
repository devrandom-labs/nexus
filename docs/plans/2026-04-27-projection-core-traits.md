# Projection Core Traits Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Define the trait-based foundation for projections — `Projector`, `StateStore`, `ProjectionTrigger`, and data types — as a parallel module to `snapshot/`, closing #155.

**Architecture:** Copy the `snapshot/` module structure into a new `projection/` module, rename types, and add the one genuinely new trait (`Projector`). Both modules coexist until subtask 5 (#159) unifies them. Feature-gated behind `projection` flag.

**Tech Stack:** Rust (edition 2024), nexus-store crate, tokio (testing only), proptest

---

### Task 1: Add `projection` feature flag

**Files:**
- Modify: `crates/nexus-store/Cargo.toml`

**Step 1: Add the feature flags**

In `crates/nexus-store/Cargo.toml`, add to the `[features]` section:

```toml
projection = []
projection-json = ["projection", "json"]
```

**Step 2: Verify it compiles**

Run: `cargo check -p nexus-store --features projection`
Expected: PASS (no projection code yet, feature just exists)

**Step 3: Commit**

```bash
git add crates/nexus-store/Cargo.toml
git commit -m "feat(store): add projection feature flags"
```

---

### Task 2: Create `PendingState` and `PersistedState` types

**Files:**
- Create: `crates/nexus-store/src/projection/pending.rs`
- Create: `crates/nexus-store/src/projection/persisted.rs`
- Create: `crates/nexus-store/src/projection/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Create: `crates/nexus-store/tests/projection_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/projection_tests.rs`:

```rust
#![cfg(feature = "projection")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::as_conversions,
    reason = "test harness — relaxed lints for test code"
)]

use std::num::NonZeroU32;

use nexus::Version;
use nexus_store::projection::{PendingState, PersistedState};

#[test]
fn pending_state_stores_version_and_payload() {
    let version = Version::new(42).unwrap();
    let payload = vec![1, 2, 3];
    let sv = NonZeroU32::new(1).unwrap();
    let state = PendingState::new(version, sv, payload.clone());

    assert_eq!(state.version(), version);
    assert_eq!(state.schema_version(), sv);
    assert_eq!(state.payload(), &payload);
}

#[test]
fn persisted_state_stores_version_and_payload() {
    let version = Version::new(10).unwrap();
    let sv = NonZeroU32::new(2).unwrap();
    let state = PersistedState::new(version, sv, vec![4, 5, 6]);

    assert_eq!(state.version(), version);
    assert_eq!(state.schema_version(), sv);
    assert_eq!(state.payload(), &[4, 5, 6]);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: FAIL — module `projection` not found

**Step 3: Create the projection module and types**

Create `crates/nexus-store/src/projection/pending.rs` — copy from `snapshot/pending.rs`, rename `PendingSnapshot` → `PendingState`:

```rust
use std::num::NonZeroU32;

use nexus::Version;

/// Projection state payload ready for persistence (write path).
///
/// Contains the serialized projection state, the stream version at
/// state computation time, and a schema version for invalidation.
#[derive(Debug, Clone)]
pub struct PendingState {
    version: Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl PendingState {
    /// Create a new pending projection state.
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, payload: Vec<u8>) -> Self {
        Self {
            version,
            schema_version,
            payload,
        }
    }

    /// The stream version at the time of state computation.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
```

Create `crates/nexus-store/src/projection/persisted.rs` — copy from `snapshot/persisted.rs`, rename `PersistedSnapshot` → `PersistedState`:

```rust
use std::num::NonZeroU32;

use nexus::Version;

/// Persisted projection state loaded from storage (read path).
///
/// Owns the payload bytes. A borrowing variant with GAT lifetimes
/// can be added later if benchmarks justify it.
#[derive(Debug, Clone)]
pub struct PersistedState {
    version: Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl PersistedState {
    /// Create a new persisted projection state.
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, payload: Vec<u8>) -> Self {
        Self {
            version,
            schema_version,
            payload,
        }
    }

    /// The stream version at the time of state computation.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
```

Create `crates/nexus-store/src/projection/mod.rs`:

```rust
mod pending;
mod persisted;

pub use pending::PendingState;
pub use persisted::PersistedState;
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(feature = "projection")]
pub mod projection;
```

And re-exports:

```rust
#[cfg(feature = "projection")]
pub use projection::{PendingState, PersistedState};
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/projection/ crates/nexus-store/src/lib.rs crates/nexus-store/tests/projection_tests.rs
git commit -m "feat(store): add PendingState and PersistedState for projection module"
```

---

### Task 3: Add `StateStore` trait

**Files:**
- Create: `crates/nexus-store/src/projection/store.rs`
- Modify: `crates/nexus-store/src/projection/mod.rs`
- Modify: `crates/nexus-store/tests/projection_tests.rs`

**Step 1: Write failing tests**

Append to `crates/nexus-store/tests/projection_tests.rs`:

```rust
use nexus_store::projection::StateStore;

// ── () no-op StateStore ─────────────────────────────────────────

#[tokio::test]
async fn unit_state_store_returns_none() {
    let store = ();
    let id = TestId("proj-1".into());
    let result = store.load(&id, SV1).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn unit_state_store_save_succeeds() {
    let store = ();
    let id = TestId("proj-1".into());
    let state = PendingState::new(
        Version::new(1).unwrap(),
        NonZeroU32::new(1).unwrap(),
        vec![1, 2, 3],
    );
    let result = store.save(&id, &state).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn unit_state_store_delete_succeeds() {
    let store = ();
    let id = TestId("proj-1".into());
    let result = store.delete(&id).await;
    assert!(result.is_ok());
}
```

Also add the `TestId` and `SV1` definitions at the top of the file (same pattern as snapshot_tests.rs):

```rust
use std::fmt;

const SV1: NonZeroU32 = NonZeroU32::MIN;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl nexus::Id for TestId {
    const BYTE_LEN: usize = 0;
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: FAIL — `StateStore` not found

**Step 3: Create the StateStore trait**

Create `crates/nexus-store/src/projection/store.rs` — copy from `snapshot/store.rs`, rename:

```rust
use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::Id;

use super::pending::PendingState;
use super::persisted::PersistedState;

/// Byte-level projection state storage trait.
///
/// Adapters (fjall, postgres, etc.) implement this to persist and
/// retrieve serialized projection state.
///
/// `()` is the no-op implementation: `load` always returns `None`,
/// `save` and `delete` silently discard. Used when projection state
/// storage is not configured.
pub trait StateStore: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the most recent state for the given projection.
    ///
    /// Returns `None` if no state exists or if the stored state's
    /// schema version does not match `schema_version`. Filtering at the
    /// store level avoids loading payload bytes for stale state.
    fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<PersistedState>, Self::Error>> + Send;

    /// Persist projection state.
    ///
    /// Overwrites any existing state for this projection.
    fn save(
        &self,
        id: &impl Id,
        state: &PendingState,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete projection state.
    ///
    /// Returns `Ok(())` if no state exists (idempotent).
    fn delete(&self, id: &impl Id) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: StateStore> StateStore for &T {
    type Error = T::Error;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Self::Error> {
        (**self).load(id, schema_version).await
    }

    async fn save(&self, id: &impl Id, state: &PendingState) -> Result<(), Self::Error> {
        (**self).save(id, state).await
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        (**self).delete(id).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — projection state storage disabled
// ═══════════════════════════════════════════════════════════════════════════

impl StateStore for () {
    type Error = Infallible;

    async fn load(
        &self,
        _id: &impl Id,
        _schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _state: &PendingState) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}
```

Update `crates/nexus-store/src/projection/mod.rs`:

```rust
mod pending;
mod persisted;
mod store;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use store::StateStore;
```

Add re-export in `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(feature = "projection")]
pub use projection::{PendingState, PersistedState, StateStore};
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/projection/store.rs crates/nexus-store/src/projection/mod.rs crates/nexus-store/src/lib.rs crates/nexus-store/tests/projection_tests.rs
git commit -m "feat(store): add StateStore trait for projection state persistence"
```

---

### Task 4: Add `ProjectionTrigger` with `EveryNEvents` and `AfterEventTypes`

**Files:**
- Create: `crates/nexus-store/src/projection/trigger.rs`
- Modify: `crates/nexus-store/src/projection/mod.rs`
- Modify: `crates/nexus-store/tests/projection_tests.rs`

**Step 1: Write failing tests**

Append to `crates/nexus-store/tests/projection_tests.rs`:

```rust
use std::num::NonZeroU64;

use nexus_store::projection::{
    AfterEventTypes as ProjAfterEventTypes, EveryNEvents as ProjEveryNEvents, ProjectionTrigger,
};

// ── EveryNEvents ────────────────────────────────────────────────────

#[test]
fn proj_every_n_events_triggers_on_boundary_crossing() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_project(v99, v100, std::iter::empty::<&str>()));

    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_project(v98, v99_ver, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_project(old, new, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_first_save_triggers_at_boundary() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let new = Version::new(100).unwrap();
    assert!(trigger.should_project(None, new, std::iter::empty::<&str>()));

    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_project(None, new_50, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_1_event_always_triggers() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_project(None, Version::new(1).unwrap(), std::iter::empty::<&str>()));
    assert!(trigger.should_project(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        std::iter::empty::<&str>(),
    ));
}

// ── AfterEventTypes ─────────────────────────────────────────────────

#[test]
fn proj_after_event_types_triggers_on_matching_event() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted", "OrderCancelled"]);

    assert!(trigger.should_project(None, Version::new(5).unwrap(), ["OrderCompleted"].into_iter()));
    assert!(trigger.should_project(
        None,
        Version::new(5).unwrap(),
        ["OrderCancelled"].into_iter()
    ));
    assert!(!trigger.should_project(None, Version::new(5).unwrap(), ["ItemAdded"].into_iter()));
}

#[test]
fn proj_after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_project(
        None,
        Version::new(5).unwrap(),
        ["ItemAdded", "OrderCompleted"].into_iter(),
    ));
}

#[test]
fn proj_after_event_types_does_not_trigger_on_empty_events() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_project(None, Version::new(5).unwrap(), std::iter::empty::<&str>()));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: FAIL — `ProjectionTrigger` not found

**Step 3: Create the trigger trait and implementations**

Create `crates/nexus-store/src/projection/trigger.rs` — copy from `snapshot/trigger.rs`, rename `SnapshotTrigger` → `ProjectionTrigger`, `should_snapshot` → `should_project`:

```rust
use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to update projection state.
///
/// Checked after each successful `save()`. The trigger receives both the
/// old and new version (to detect boundary crossings regardless of batch
/// size) and the names of events just persisted (for semantic triggers).
pub trait ProjectionTrigger: Send + Sync {
    /// Whether a projection state update should happen after this save.
    ///
    /// - `old_version`: version before save (`None` for new streams)
    /// - `new_version`: version after save
    /// - `event_names`: names of events just persisted (from `DomainEvent::name()`).
    fn should_project(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Trigger a projection state update every N events.
///
/// Uses a bucket-crossing algorithm to detect when a save crosses an
/// N-event boundary, regardless of batch size.
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl ProjectionTrigger for EveryNEvents {
    fn should_project(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        _event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        let n = self.0.get();
        let old_bucket = old_version.map_or(0, |v| v.as_u64() / n);
        let new_bucket = new_version.as_u64() / n;
        new_bucket > old_bucket
    }
}

/// Trigger a projection state update after specific event types.
///
/// Useful for domain milestones where a projection update aligns with
/// business semantics.
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

impl ProjectionTrigger for AfterEventTypes {
    fn should_project(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        mut event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}
```

Update `crates/nexus-store/src/projection/mod.rs`:

```rust
mod pending;
mod persisted;
mod store;
mod trigger;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use store::StateStore;
pub use trigger::{AfterEventTypes, EveryNEvents, ProjectionTrigger};
```

Update re-exports in `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(feature = "projection")]
pub use projection::{
    AfterEventTypes as ProjAfterEventTypes, EveryNEvents as ProjEveryNEvents, PendingState,
    PersistedState, ProjectionTrigger, StateStore,
};
```

**Note:** The trigger type names (`EveryNEvents`, `AfterEventTypes`) collide with the snapshot re-exports. Use aliased re-exports at the crate root. The `projection::` module path remains unaliased for direct use.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/projection/trigger.rs crates/nexus-store/src/projection/mod.rs crates/nexus-store/src/lib.rs crates/nexus-store/tests/projection_tests.rs
git commit -m "feat(store): add ProjectionTrigger trait with EveryNEvents and AfterEventTypes"
```

---

### Task 5: Add `InMemoryStateStore` (testing)

**Files:**
- Create: `crates/nexus-store/src/projection/testing.rs`
- Modify: `crates/nexus-store/src/projection/mod.rs`
- Modify: `crates/nexus-store/tests/projection_tests.rs`

**Step 1: Write failing tests**

Append to `crates/nexus-store/tests/projection_tests.rs`:

```rust
// ── InMemoryStateStore ───────────────────────────────────────────

#[cfg(feature = "testing")]
mod in_memory_tests {
    use super::*;
    use nexus_store::projection::InMemoryStateStore;

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let store = InMemoryStateStore::new();
        let result = store.load(&TestId("proj-1".into()), SV1).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = InMemoryStateStore::new();
        let id = TestId("proj-1".into());
        let version = Version::new(10).unwrap();
        let state = PendingState::new(version, NonZeroU32::new(1).unwrap(), vec![1, 2, 3]);

        store.save(&id, &state).await.unwrap();
        let loaded = store.load(&id, SV1).await.unwrap().unwrap();

        assert_eq!(loaded.version(), version);
        assert_eq!(loaded.schema_version(), NonZeroU32::new(1).unwrap());
        assert_eq!(loaded.payload(), &[1, 2, 3]);
    }

    #[tokio::test]
    async fn save_overwrites_previous_state() {
        let store = InMemoryStateStore::new();
        let id = TestId("proj-1".into());

        let state1 = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store.save(&id, &state1).await.unwrap();

        let state2 = PendingState::new(
            Version::new(20).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![2],
        );
        store.save(&id, &state2).await.unwrap();

        let loaded = store.load(&id, SV1).await.unwrap().unwrap();
        assert_eq!(loaded.version(), Version::new(20).unwrap());
        assert_eq!(loaded.payload(), &[2]);
    }

    #[tokio::test]
    async fn different_projections_have_separate_state() {
        let store = InMemoryStateStore::new();

        let state1 = PendingState::new(
            Version::new(5).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store
            .save(&TestId("proj-1".into()), &state1)
            .await
            .unwrap();

        let state2 = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![2],
        );
        store
            .save(&TestId("proj-2".into()), &state2)
            .await
            .unwrap();

        let loaded1 = store
            .load(&TestId("proj-1".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        let loaded2 = store
            .load(&TestId("proj-2".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.version(), Version::new(5).unwrap());
        assert_eq!(loaded2.version(), Version::new(10).unwrap());
    }

    #[tokio::test]
    async fn delete_removes_state() {
        let store = InMemoryStateStore::new();
        let id = TestId("proj-1".into());

        let state = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store.save(&id, &state).await.unwrap();

        store.delete(&id).await.unwrap();
        let loaded = store.load(&id, SV1).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let store = InMemoryStateStore::new();
        let result = store.delete(&TestId("nope".into())).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn load_filters_by_schema_version() {
        let store = InMemoryStateStore::new();
        let id = TestId("proj-1".into());

        let state = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1, 2, 3],
        );
        store.save(&id, &state).await.unwrap();

        // Matching schema version returns state
        let loaded = store.load(&id, NonZeroU32::new(1).unwrap()).await.unwrap();
        assert!(loaded.is_some());

        // Mismatched schema version returns None
        let loaded = store.load(&id, NonZeroU32::new(2).unwrap()).await.unwrap();
        assert!(loaded.is_none());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features "projection,testing" -- projection_tests`
Expected: FAIL — `InMemoryStateStore` not found

**Step 3: Create the InMemoryStateStore**

Create `crates/nexus-store/src/projection/testing.rs` — copy from `snapshot/testing.rs`, rename:

```rust
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingState;
use super::persisted::PersistedState;
use super::store::StateStore;

/// In-memory projection state store for tests.
///
/// Stores projection state in a `HashMap` protected by a `RwLock`.
#[derive(Debug, Default)]
pub struct InMemoryStateStore {
    states: RwLock<HashMap<String, StoredState>>,
}

#[derive(Debug, Clone)]
struct StoredState {
    version: nexus::Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl InMemoryStateStore {
    /// Create a new empty in-memory state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl StateStore for InMemoryStateStore {
    type Error = Infallible;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Infallible> {
        let states = self.states.read().await;
        let key = id.to_string();
        Ok(states
            .get(&key)
            .filter(|s| s.schema_version == schema_version)
            .map(|s| PersistedState::new(s.version, s.schema_version, s.payload.clone())))
    }

    async fn save(&self, id: &impl Id, state: &PendingState) -> Result<(), Infallible> {
        let mut states = self.states.write().await;
        let key = id.to_string();
        states.insert(
            key,
            StoredState {
                version: state.version(),
                schema_version: state.schema_version(),
                payload: state.payload().to_vec(),
            },
        );
        Ok(())
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Infallible> {
        let mut states = self.states.write().await;
        states.remove(&id.to_string());
        Ok(())
    }
}
```

Update `crates/nexus-store/src/projection/mod.rs`:

```rust
mod pending;
mod persisted;
mod store;
#[cfg(feature = "testing")]
mod testing;
mod trigger;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use store::StateStore;
#[cfg(feature = "testing")]
pub use testing::InMemoryStateStore;
pub use trigger::{AfterEventTypes, EveryNEvents, ProjectionTrigger};
```

Update re-exports in `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(all(feature = "projection", feature = "testing"))]
pub use projection::InMemoryStateStore;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store --features "projection,testing" -- projection_tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/projection/testing.rs crates/nexus-store/src/projection/mod.rs crates/nexus-store/src/lib.rs crates/nexus-store/tests/projection_tests.rs
git commit -m "feat(store): add InMemoryStateStore for projection testing"
```

---

### Task 6: Add `Projector` trait

**Files:**
- Create: `crates/nexus-store/src/projection/projector.rs`
- Modify: `crates/nexus-store/src/projection/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Modify: `crates/nexus-store/tests/projection_tests.rs`

**Step 1: Write failing tests**

Append to `crates/nexus-store/tests/projection_tests.rs`:

```rust
use nexus_store::projection::Projector;

// ── Projector trait ─────────────────────────────────────────────────

/// A test projector that counts events and sums a field.
struct CountingProjector;

#[derive(Debug, Clone, PartialEq)]
struct CountState {
    count: u64,
    total: u64,
}

#[derive(Debug)]
enum TestEvent {
    Added(u64),
    Removed(u64),
}

impl nexus::Message for TestEvent {}
impl nexus::DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Added(_) => "Added",
            TestEvent::Removed(_) => "Removed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("overflow")]
struct ProjectionError;

impl Projector for CountingProjector {
    type Event = TestEvent;
    type State = CountState;
    type Error = ProjectionError;

    fn initial(&self) -> CountState {
        CountState { count: 0, total: 0 }
    }

    fn apply(&self, state: CountState, event: &TestEvent) -> Result<CountState, ProjectionError> {
        match event {
            TestEvent::Added(n) => Ok(CountState {
                count: state.count.checked_add(1).ok_or(ProjectionError)?,
                total: state.total.checked_add(*n).ok_or(ProjectionError)?,
            }),
            TestEvent::Removed(n) => Ok(CountState {
                count: state.count.checked_add(1).ok_or(ProjectionError)?,
                total: state.total.checked_sub(*n).ok_or(ProjectionError)?,
            }),
        }
    }
}

#[test]
fn projector_initial_state() {
    let proj = CountingProjector;
    let state = proj.initial();
    assert_eq!(state, CountState { count: 0, total: 0 });
}

#[test]
fn projector_folds_events() {
    let proj = CountingProjector;
    let state = proj.initial();

    let state = proj.apply(state, &TestEvent::Added(10)).unwrap();
    assert_eq!(state, CountState { count: 1, total: 10 });

    let state = proj.apply(state, &TestEvent::Added(20)).unwrap();
    assert_eq!(state, CountState { count: 2, total: 30 });

    let state = proj.apply(state, &TestEvent::Removed(5)).unwrap();
    assert_eq!(state, CountState { count: 3, total: 25 });
}

#[test]
fn projector_returns_error_on_overflow() {
    let proj = CountingProjector;
    let state = CountState {
        count: 0,
        total: u64::MAX,
    };
    let result = proj.apply(state, &TestEvent::Added(1));
    assert!(result.is_err());
}

#[test]
fn projector_returns_error_on_underflow() {
    let proj = CountingProjector;
    let state = CountState { count: 0, total: 0 };
    let result = proj.apply(state, &TestEvent::Removed(1));
    assert!(result.is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: FAIL — `Projector` not found

**Step 3: Create the Projector trait**

Create `crates/nexus-store/src/projection/projector.rs`:

```rust
use nexus::DomainEvent;

/// A pure fold function over domain events.
///
/// Processes events one at a time to produce derived state. The
/// framework handles all IO (reading events, persisting state,
/// checkpointing). The projector is only responsible for computation.
///
/// Fallible: `apply` returns `Result` because projections may perform
/// checked arithmetic or encounter domain-specific edge cases.
/// Recovery policy (skip, fail, dead-letter) is handled by middleware
/// layers, not the projector itself.
///
/// # Comparison with `AggregateState`
///
/// `AggregateState::apply` is infallible because an aggregate always
/// applies its own events. A projector may process events from any
/// source, and may do derived computations (sums, counts) that can
/// overflow.
pub trait Projector: Send + Sync + 'static {
    /// The domain event type this projector handles.
    type Event: DomainEvent;

    /// The derived state produced by folding events.
    type State: Send + Sync + 'static;

    /// Error type for fallible projection logic.
    type Error: core::error::Error + Send + Sync + 'static;

    /// The initial state before any events have been applied.
    fn initial(&self) -> Self::State;

    /// Apply a single event to the current state, producing new state.
    ///
    /// Must use checked arithmetic for all computations. Return `Err`
    /// on overflow, underflow, or any domain-specific invariant
    /// violation. The framework decides recovery policy.
    fn apply(&self, state: Self::State, event: &Self::Event) -> Result<Self::State, Self::Error>;
}
```

Update `crates/nexus-store/src/projection/mod.rs`:

```rust
mod pending;
mod persisted;
mod projector;
mod store;
#[cfg(feature = "testing")]
mod testing;
mod trigger;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use projector::Projector;
pub use store::StateStore;
#[cfg(feature = "testing")]
pub use testing::InMemoryStateStore;
pub use trigger::{AfterEventTypes, EveryNEvents, ProjectionTrigger};
```

Update re-exports in `crates/nexus-store/src/lib.rs`:

```rust
#[cfg(feature = "projection")]
pub use projection::{
    AfterEventTypes as ProjAfterEventTypes, EveryNEvents as ProjEveryNEvents, PendingState,
    PersistedState, ProjectionTrigger, Projector, StateStore,
};
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store --features projection -- projection_tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/projection/projector.rs crates/nexus-store/src/projection/mod.rs crates/nexus-store/src/lib.rs crates/nexus-store/tests/projection_tests.rs
git commit -m "feat(store): add Projector trait — pure event fold with fallible apply"
```

---

### Task 7: Run full check and update hakari

**Files:**
- Possibly modify: `crates/workspace-hack/Cargo.toml`

**Step 1: Generate hakari**

Run: `cargo hakari generate`

If it changes `crates/workspace-hack/Cargo.toml`, stage and include in the commit.

**Step 2: Run clippy**

Run: `cargo clippy -p nexus-store --features "projection,testing" -- -D warnings`
Expected: PASS — no warnings

**Step 3: Run fmt**

Run: `cargo fmt --all -- --check`
Expected: PASS

**Step 4: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS — no regressions. Existing snapshot tests unaffected.

**Step 5: Run tests with all feature combinations**

Run: `cargo test -p nexus-store --features "projection"`
Run: `cargo test -p nexus-store --features "projection,testing"`
Run: `cargo test -p nexus-store --features "projection,snapshot"`
Run: `cargo test -p nexus-store --features "projection,snapshot,testing"`
Expected: All PASS — no feature interaction issues

**Step 6: Commit**

```bash
git add -A
git commit -m "chore: update workspace-hack after projection feature"
```

---

### Task 8: Final review and PR

**Step 1: Review the complete diff**

Run: `git diff main --stat`

Verify:
- Only `crates/nexus-store/` files changed (plus workspace-hack if applicable)
- No changes to `snapshot/` module
- New `projection/` module with 6 files: `mod.rs`, `pending.rs`, `persisted.rs`, `store.rs`, `trigger.rs`, `projector.rs`, `testing.rs`
- Feature flags added to `Cargo.toml`
- Re-exports added to `lib.rs`
- Tests in `projection_tests.rs`

**Step 2: Create PR**

PR title: `feat(store): projection core traits (#155)`

Body should reference:
- Parent issue #124
- Subtask #155
- That `SnapshotStore` is intentionally untouched (coexistence until #159)
- That `Projector` is the one genuinely new trait
- Feature flags: `projection`, `projection-json`
