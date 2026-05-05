# Unified State Persistence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Collapse 4 identical byte-level types and 2 identical traits into 2 generic types (`PendingState<S>`, `PersistedState<S>`) and 1 unified trait (`StateStore<S>`), pushing codec responsibility into adapters.

**Architecture:** `PendingState<S>` and `PersistedState<S>` become generic containers (state + version + schema_version). `StateStore<S>` is a generic trait — adapters implement it for their native serialization format (fjall: `StateStore<Vec<u8>>`, postgres: `StateStore<S> where S: FromRow`). The framework's `StatePersistence` wrapper disappears — codec moves into adapter construction. Both projections and snapshots use the same types and trait.

**Tech Stack:** Rust, nexus-store crate, nexus-framework crate, nexus-fjall crate

---

## Current State (What We're Killing)

4 identical structs (`{ version: Version, schema_version: NonZeroU32, payload: Vec<u8> }`):
- `projection/pending.rs` → `PendingState`
- `projection/persisted.rs` → `PersistedState`
- `snapshot/pending.rs` → `PendingSnapshot`
- `snapshot/persisted.rs` → `PersistedSnapshot`

2 identical traits (`load(id, schema_version)`, `save(id, &state)`, `delete(id)`):
- `projection/store.rs` → `StateStore`
- `snapshot/store.rs` → `SnapshotStore`

2 identical trigger traits (`should_*(old, new, events) -> bool`) + 2 identical impls:
- `projection/trigger.rs` → `ProjectionTrigger`, `EveryNEvents`, `AfterEventTypes`
- `snapshot/trigger.rs` → `SnapshotTrigger`, `EveryNEvents`, `AfterEventTypes`

`lib.rs` already has aliased re-exports (`ProjAfterEventTypes`, `ProjEveryNEvents`) because the names collide.

## Target State

```
nexus-store/src/
├── state/              # NEW unified module
│   ├── mod.rs          # re-exports
│   ├── pending.rs      # PendingState<S>
│   ├── persisted.rs    # PersistedState<S>
│   ├── store.rs        # StateStore<S> trait + () no-op + &T delegation
│   ├── trigger.rs      # PersistTrigger trait + EveryNEvents + AfterEventTypes
│   └── testing.rs      # InMemoryStateStore<S> (feature-gated)
├── projection/         # SLIMMED — projector only
│   ├── mod.rs
│   └── projector.rs    # Projector trait (unchanged)
├── snapshot/           # DELETED entirely
├── store/
│   └── snapshot/
│       └── snapshotting.rs  # Uses StateStore<A::State> directly (no codec field)
```

## Key Design Decisions

1. **`StateStore<S>` uses generic parameter, not associated type** — allows `impl<S> StateStore<S> for ()` (no-op for any type) and `impl StateStore<Vec<u8>> for FjallStore` without conflict.

2. **Codec moves into adapter** — Fjall adapter wraps `FjallStore + Codec<S>` internally and implements `StateStore<S>`. Framework no longer owns codec for state persistence.

3. **`schema_version` stays in the trait signature** — it's a query filter, not a codec concern. Same store may hold multiple schema versions during migrations.

4. **Feature gate**: single `state` feature replaces both `projection` and `snapshot`. `projection` feature now only gates `Projector`. `snapshot` feature becomes an alias or is removed.

5. **Triggers unified** — `PersistTrigger` replaces both `ProjectionTrigger` and `SnapshotTrigger`. Same `EveryNEvents` and `AfterEventTypes` impls (they were copy-paste).

---

## Phase 1: Create Unified Types + Trait (nexus-store)

### Task 1: Create `state/` module with generic `PendingState<S>`

**Files:**
- Create: `crates/nexus-store/src/state/mod.rs`
- Create: `crates/nexus-store/src/state/pending.rs`

**Step 1: Write `state/pending.rs`**

```rust
use std::num::NonZeroU32;

use nexus::Version;

/// State payload ready for persistence (write path).
///
/// Generic over `S` — the adapter decides serialization format.
/// For byte-level adapters (fjall): `PendingState<Vec<u8>>`.
/// For typed adapters (postgres): `PendingState<MyState>`.
#[derive(Debug, Clone)]
pub struct PendingState<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> PendingState<S> {
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, state: S) -> Self {
        Self { version, schema_version, state }
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    /// Consume, returning parts.
    #[must_use]
    pub fn into_parts(self) -> (Version, NonZeroU32, S) {
        (self.version, self.schema_version, self.state)
    }
}
```

**Step 2: Write `state/mod.rs` (initially just pending)**

```rust
mod pending;

pub use pending::PendingState;
```

**Step 3: Wire into `lib.rs`**

Add `pub mod state;` (no feature gate — these are fundamental types).

**Step 4: Run `cargo check -p nexus-store`**

Expected: compiles cleanly.

**Step 5: Commit**

```
feat(store): add generic PendingState<S> in state/ module
```

---

### Task 2: Add generic `PersistedState<S>`

**Files:**
- Create: `crates/nexus-store/src/state/persisted.rs`
- Modify: `crates/nexus-store/src/state/mod.rs`

**Step 1: Write `state/persisted.rs`**

```rust
use std::num::NonZeroU32;

use nexus::Version;

/// Persisted state loaded from storage (read path).
///
/// Generic over `S` — byte-level adapters return `PersistedState<Vec<u8>>`,
/// typed adapters return `PersistedState<MyState>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedState<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> PersistedState<S> {
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, state: S) -> Self {
        Self { version, schema_version, state }
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    /// Consume, returning parts.
    #[must_use]
    pub fn into_parts(self) -> (Version, NonZeroU32, S) {
        (self.version, self.schema_version, self.state)
    }
}
```

**Step 2: Update `state/mod.rs`**

```rust
mod pending;
mod persisted;

pub use pending::PendingState;
pub use persisted::PersistedState;
```

**Step 3: `cargo check -p nexus-store`**

**Step 4: Commit**

```
feat(store): add generic PersistedState<S> in state/ module
```

---

### Task 3: Add unified `StateStore<S>` trait

**Files:**
- Create: `crates/nexus-store/src/state/store.rs`
- Modify: `crates/nexus-store/src/state/mod.rs`

**Step 1: Write `state/store.rs`**

```rust
use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::Id;

use super::pending::PendingState;
use super::persisted::PersistedState;

/// Versioned state persistence trait.
///
/// Unified trait for both projection state and aggregate snapshots.
/// Generic over `S` — the adapter decides how to serialize.
///
/// `()` is the no-op implementation: `load` returns `None`,
/// `save` and `delete` silently discard.
pub trait StateStore<S>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load persisted state, filtering by schema version.
    ///
    /// Returns `None` if no state exists or schema version mismatches.
    fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<PersistedState<S>>, Self::Error>> + Send;

    /// Persist state (overwrites existing).
    fn save(
        &self,
        id: &impl Id,
        state: &PendingState<S>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete persisted state (idempotent).
    fn delete(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// No-op implementation
impl<S: Send + Sync> StateStore<S> for () {
    type Error = Infallible;

    async fn load(
        &self,
        _id: &impl Id,
        _schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState<S>>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _state: &PendingState<S>) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}

// Delegation for shared references
impl<S: Send + Sync, T: StateStore<S>> StateStore<S> for &T {
    type Error = T::Error;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState<S>>, Self::Error> {
        (**self).load(id, schema_version).await
    }

    async fn save(&self, id: &impl Id, state: &PendingState<S>) -> Result<(), Self::Error> {
        (**self).save(id, state).await
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        (**self).delete(id).await
    }
}
```

**Step 2: Update `state/mod.rs`**

```rust
mod pending;
mod persisted;
mod store;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use store::StateStore;
```

**Step 3: `cargo check -p nexus-store`**

**Step 4: Commit**

```
feat(store): add unified StateStore<S> trait in state/ module
```

---

### Task 4: Add unified `PersistTrigger` trait

**Files:**
- Create: `crates/nexus-store/src/state/trigger.rs`
- Modify: `crates/nexus-store/src/state/mod.rs`

**Step 1: Write `state/trigger.rs`**

```rust
use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to persist state.
///
/// Used by both projection runners (when to checkpoint projection state)
/// and snapshot decorators (when to snapshot aggregate state).
pub trait PersistTrigger: Send + Sync {
    /// Whether state should be persisted now.
    ///
    /// - `old_version`: version before the operation (`None` for first run)
    /// - `new_version`: version after the operation
    /// - `event_names`: names of events just processed
    fn should_persist(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Persist every N events (bucket-crossing algorithm).
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl PersistTrigger for EveryNEvents {
    fn should_persist(
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

/// Persist after specific event types.
#[derive(Debug, Clone)]
pub struct AfterEventTypes {
    types: Vec<&'static str>,
}

impl AfterEventTypes {
    #[must_use]
    pub fn new(types: &[&'static str]) -> Self {
        Self { types: types.to_vec() }
    }
}

impl PersistTrigger for AfterEventTypes {
    fn should_persist(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        mut event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}
```

**Step 2: Update `state/mod.rs`**

```rust
mod pending;
mod persisted;
mod store;
mod trigger;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use store::StateStore;
pub use trigger::{AfterEventTypes, EveryNEvents, PersistTrigger};
```

**Step 3: `cargo check -p nexus-store`**

**Step 4: Commit**

```
feat(store): add unified PersistTrigger trait in state/ module
```

---

### Task 5: Add `InMemoryStateStore<S>` for testing

**Files:**
- Create: `crates/nexus-store/src/state/testing.rs`
- Modify: `crates/nexus-store/src/state/mod.rs`

**Step 1: Write `state/testing.rs`**

```rust
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingState;
use super::persisted::PersistedState;
use super::store::StateStore;

/// In-memory state store for tests.
#[derive(Debug, Default)]
pub struct InMemoryStateStore<S> {
    states: RwLock<HashMap<String, PersistedState<S>>>,
}

impl<S> InMemoryStateStore<S> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }
}

impl<S: Clone + Send + Sync + 'static> StateStore<S> for InMemoryStateStore<S> {
    type Error = Infallible;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState<S>>, Infallible> {
        let states = self.states.read().await;
        let key = id.to_string();
        Ok(states
            .get(&key)
            .filter(|s| s.schema_version() == schema_version)
            .cloned())
    }

    async fn save(&self, id: &impl Id, state: &PendingState<S>) -> Result<(), Infallible>
    where
        S: Clone,
    {
        let key = id.to_string();
        let persisted = PersistedState::new(
            state.version(),
            state.schema_version(),
            state.state().clone(),
        );
        self.states.write().await.insert(key, persisted);
        Ok(())
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Infallible> {
        self.states.write().await.remove(&id.to_string());
        Ok(())
    }
}
```

**Step 2: Update `state/mod.rs` with feature gate**

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
pub use trigger::{AfterEventTypes, EveryNEvents, PersistTrigger};
```

**Step 3: `cargo check -p nexus-store --features testing`**

**Step 4: Commit**

```
feat(store): add InMemoryStateStore<S> for testing
```

---

## Phase 2: Migrate Consumers

### Task 6: Migrate `Snapshotting` to unified `StateStore<S>`

**Files:**
- Modify: `crates/nexus-store/src/store/snapshot/snapshotting.rs`

**Key change:** Remove `snapshot_codec: SC` field. `SS` bound changes from `SnapshotStore` (byte-level) to `StateStore<A::State>` (typed). The adapter handles codec internally.

**Step 1: Rewrite `Snapshotting` struct**

```rust
pub struct Snapshotting<R, SS, T> {
    inner: R,
    state_store: SS,
    trigger: T,
    schema_version: NonZeroU32,
    snapshot_on_read: bool,
}
```

**Step 2: Rewrite `Repository<A>` impl**

`SS: StateStore<A::State>` instead of `SS: SnapshotStore` + `SC: Codec<A::State>`.

- `try_load_from_snapshot` → calls `self.state_store.load(id, schema_version)`, gets back `PersistedState<A::State>` directly (no decode needed).
- `try_save_snapshot` → wraps state in `PendingState::new(version, schema_version, state.clone())`, calls `self.state_store.save(id, &pending)`.

**Step 3: Update `store/snapshot/mod.rs` re-exports**

**Step 4: `cargo check -p nexus-store --features snapshot`**

**Step 5: Commit**

```
refactor(store): migrate Snapshotting to unified StateStore<S>
```

---

### Task 7: Migrate `nexus-fjall` SnapshotStore impl

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`

**Key change:** Fjall implements `StateStore<Vec<u8>>` — it stores raw bytes. The codec wrapping moves to a separate adapter type constructed by the user. Alternatively, a `CodecStateStore<SS, C, S>` wrapper (in nexus-store or nexus-fjall) that implements `StateStore<S>` by encoding/decoding through a `StateStore<Vec<u8>>` + `Codec<S>`.

**Design choice:** A `CodecStateStore` adapter in nexus-store is cleanest — it's reusable for any byte-level backend:

```rust
// In nexus-store/src/state/codec_adapter.rs
pub struct CodecStateStore<SS, C> {
    store: SS,
    codec: C,
}

impl<S, SS, C> StateStore<S> for CodecStateStore<SS, C>
where
    SS: StateStore<Vec<u8>>,
    C: Codec<S>,
    S: Send + Sync,
{
    // encode on save, decode on load
}
```

**Step 1: Add `CodecStateStore` adapter to `state/` module**

**Step 2: Fjall implements `StateStore<Vec<u8>>` (minimal change from current `SnapshotStore` impl)**

**Step 3: Update fjall tests**

**Step 4: `cargo check -p nexus-fjall`**

**Step 5: Commit**

```
refactor(fjall): implement unified StateStore<Vec<u8>>
```

---

### Task 8: Migrate framework `StatePersistence`

**Files:**
- Modify: `crates/nexus-framework/src/projection/persist.rs`
- Modify: `crates/nexus-framework/src/projection/projection.rs`
- Modify: `crates/nexus-framework/src/projection/builder.rs`

**Key change:** `WithStatePersistence<SS, SC>` becomes unnecessary. The framework holds a `SS: StateStore<P::State>` directly. The `StatePersistence` trait can be deleted — `StateStore<S>` from the store crate replaces it entirely.

The `schema_version` concern: the projection builder still needs to pass it to `load()` and `save()`. This stays as a field on the `Projection` struct or the builder configures it.

**Step 1: Replace `StatePersistence` trait with direct `StateStore<P::State>` usage**

**Step 2: Projection struct gains `schema_version: NonZeroU32` field**

**Step 3: Update builder to accept `StateStore<P::State>` directly**

**Step 4: Update `initialize()` — calls `state_store.load(id, schema_version)` directly**

**Step 5: Update `run()` — calls `state_store.save(id, &pending)` directly**

**Step 6: `cargo check -p nexus-framework`**

**Step 7: Update framework integration tests**

**Step 8: Commit**

```
refactor(framework): use StateStore<S> directly, remove StatePersistence
```

---

### Task 9: Migrate `ProjectionTrigger` to `PersistTrigger`

**Files:**
- Modify: `crates/nexus-framework/src/projection/status.rs`
- Modify: `crates/nexus-framework/src/projection/projection.rs`
- Modify: `crates/nexus-framework/src/projection/builder.rs`
- Modify: `crates/nexus-framework/tests/projection_tests.rs`

**Key change:** Replace `ProjectionTrigger` bounds with `PersistTrigger`. Method name changes from `should_project` to `should_persist`.

**Step 1: Update trait bounds and method calls**

**Step 2: `cargo check -p nexus-framework`**

**Step 3: Update tests**

**Step 4: Commit**

```
refactor(framework): migrate to unified PersistTrigger
```

---

### Task 10: Migrate `SnapshotTrigger` in `Snapshotting`

**Files:**
- Modify: `crates/nexus-store/src/store/snapshot/snapshotting.rs`

**Key change:** `T: SnapshotTrigger` → `T: PersistTrigger`. Method `should_snapshot` → `should_persist`.

**Step 1: Update bounds and call sites**

**Step 2: `cargo check -p nexus-store --features snapshot`**

**Step 3: Commit**

```
refactor(store): migrate Snapshotting to PersistTrigger
```

---

## Phase 3: Delete Old Code

### Task 11: Delete `snapshot/` module entirely

**Files:**
- Delete: `crates/nexus-store/src/snapshot/pending.rs`
- Delete: `crates/nexus-store/src/snapshot/persisted.rs`
- Delete: `crates/nexus-store/src/snapshot/store.rs`
- Delete: `crates/nexus-store/src/snapshot/trigger.rs`
- Delete: `crates/nexus-store/src/snapshot/testing.rs`
- Delete: `crates/nexus-store/src/snapshot/mod.rs`
- Modify: `crates/nexus-store/src/lib.rs` (remove `pub mod snapshot`, update re-exports)

**Step 1: Delete files**

**Step 2: Update `lib.rs` — remove snapshot module, re-export from `state/` instead**

**Step 3: `cargo check -p nexus-store --all-features`**

**Step 4: Commit**

```
refactor(store): delete snapshot/ module (unified into state/)
```

---

### Task 12: Delete old `projection/` state files

**Files:**
- Delete: `crates/nexus-store/src/projection/pending.rs`
- Delete: `crates/nexus-store/src/projection/persisted.rs`
- Delete: `crates/nexus-store/src/projection/store.rs`
- Delete: `crates/nexus-store/src/projection/trigger.rs`
- Delete: `crates/nexus-store/src/projection/testing.rs`
- Modify: `crates/nexus-store/src/projection/mod.rs` (slim to just Projector)

**Step 1: Delete files**

**Step 2: `projection/mod.rs` becomes:**

```rust
mod projector;

pub use projector::Projector;
```

**Step 3: Update `lib.rs` re-exports**

**Step 4: `cargo check --all -p nexus-store --all-features`**

**Step 5: Commit**

```
refactor(store): delete old projection state types (unified into state/)
```

---

### Task 13: Clean up feature gates

**Files:**
- Modify: `crates/nexus-store/Cargo.toml`
- Modify: `crates/nexus-store/src/lib.rs`

**Key change:**
- Add `state` feature (or make `state/` module always available since `PendingState<S>` is fundamental)
- `snapshot` feature gates only `Snapshotting` (the repository decorator), not types
- `projection` feature gates only `Projector`
- Remove aliased re-exports (`ProjAfterEventTypes`, `ProjEveryNEvents`)

**Step 1: Update `Cargo.toml` features**

**Step 2: Update `lib.rs` re-exports**

**Step 3: `cargo check --workspace --all-features`**

**Step 4: Commit**

```
refactor(store): clean up feature gates after state unification
```

---

### Task 14: Update all test files

**Files:**
- Modify: `crates/nexus-store/tests/snapshot_tests.rs`
- Modify: `crates/nexus-store/tests/snapshot_integration_tests.rs`
- Modify: `crates/nexus-store/tests/projection_tests.rs`
- Modify: `crates/nexus-fjall/tests/snapshot_tests.rs`
- Modify: `crates/nexus-framework/tests/projection_tests.rs`

**Key change:** Update imports from old types/traits to new unified ones. Type names change:
- `PendingSnapshot` → `PendingState<Vec<u8>>`
- `PersistedSnapshot` → `PersistedState<Vec<u8>>`
- `SnapshotStore` → `StateStore<Vec<u8>>`
- `SnapshotTrigger` → `PersistTrigger`
- `InMemorySnapshotStore` → `InMemoryStateStore<Vec<u8>>`
- `ProjectionTrigger` → `PersistTrigger`
- `.payload()` → `.state()`

**Step 1: Update imports and type usage across all test files**

**Step 2: `cargo test --workspace --all-features`**

**Step 3: Commit**

```
refactor(tests): update all tests for unified state persistence
```

---

### Task 15: Update CLAUDE.md architecture docs

**Files:**
- Modify: `CLAUDE.md`

**Key change:** Update the Store Crate section to reflect the new `state/` module and unified types. Remove references to old snapshot types.

**Step 1: Rewrite the relevant sections**

**Step 2: Commit**

```
docs: update CLAUDE.md for unified state persistence
```

---

## Phase 4: Verify

### Task 16: Full verification

**Step 1:** `cargo fmt --all`

**Step 2:** `nix flake check`

**Step 3:** Fix any remaining issues

**Step 4:** Final commit if needed

---

## Summary of Deletions

| Before | After |
|--------|-------|
| `PendingState` (bytes) | `state::PendingState<S>` |
| `PersistedState` (bytes) | `state::PersistedState<S>` |
| `PendingSnapshot` (bytes) | DELETED (same as PendingState) |
| `PersistedSnapshot` (bytes) | DELETED (same as PersistedState) |
| `StateStore` trait (bytes) | `state::StateStore<S>` |
| `SnapshotStore` trait (bytes) | DELETED (same as StateStore) |
| `ProjectionTrigger` trait | `state::PersistTrigger` |
| `SnapshotTrigger` trait | DELETED (same as PersistTrigger) |
| `InMemoryStateStore` (bytes) | `state::InMemoryStateStore<S>` |
| `InMemorySnapshotStore` (bytes) | DELETED (same as InMemoryStateStore) |
| `StatePersistence` (framework) | DELETED (StateStore<S> replaces it) |
| `WithStatePersistence` (framework) | DELETED |
| `NoStatePersistence` (framework) | DELETED (replaced by `()` impl) |
