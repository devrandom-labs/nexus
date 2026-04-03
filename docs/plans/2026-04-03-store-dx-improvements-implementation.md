# Store DX Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Improve nexus-store DX with streaming rehydration, repository trait, in-memory test store, and rename cleanup — all zero-copy friendly for IoT/mobile targets.

**Architecture:** Five changes in dependency order: (1) kernel renames/replay, (2) schema_version on PersistedEnvelope, (3) InMemoryStore, (4) Repository trait, (5) example/test migration. Each task is self-contained with its own tests and commit.

**Tech Stack:** Rust 2024 edition, nexus kernel, nexus-store, tokio async, proptest

**Design doc:** `docs/plans/2026-04-03-store-dx-improvements-design.md`

---

### Task 1: Rename `apply_event` to `apply` and `apply_events` to `apply_all` on `AggregateRoot`

**Files:**
- Modify: `crates/nexus/src/aggregate.rs` (definitions at lines 166, 171, 369, 391)

**Step 1: Rename methods in `AggregateRoot` impl**

In `crates/nexus/src/aggregate.rs`, rename:
- `pub fn apply_event(` → `pub fn apply(`
- `pub fn apply_events(` → `pub fn apply_all(`
- Update internal call in `apply_all`: `self.apply_event(event)` → `self.apply(event)`

**Step 2: Rename methods in `AggregateEntity` trait**

Same file, rename:
- `fn apply_event(` → `fn apply(`
- `fn apply_events(` → `fn apply_all(`
- Update delegation: `self.root_mut().apply_event(event)` → `self.root_mut().apply(event)`
- Update delegation: `self.root_mut().apply_events(events)` → `self.root_mut().apply_all(events)`

**Step 3: Update all doc comments and doctests in aggregate.rs**

Search the file for `apply_event` in comments and doc examples, update to `apply`.

**Step 4: Run `cargo test -p nexus` to see all compile failures**

Run: `cargo test -p nexus 2>&1 | head -80`
Expected: Many compile errors showing every call site that needs updating.

**Step 5: Fix all kernel test call sites**

Files to update (replace `apply_event` → `apply`, `apply_events` → `apply_all`):
- `crates/nexus/tests/kernel_tests/aggregate_root_tests.rs`
- `crates/nexus/tests/kernel_tests/edge_case_tests.rs`
- `crates/nexus/tests/kernel_tests/security_tests.rs`
- `crates/nexus/tests/kernel_tests/property_tests.rs`
- `crates/nexus/tests/kernel_tests/integration_test.rs`
- `crates/nexus/tests/kernel_tests/newtype_aggregate_tests.rs`
- `crates/nexus/tests/compile_fail/wrong_event_type.rs` (and update its `.stderr` snapshot)

**Step 6: Fix example call sites**

Files to update:
- `examples/inmemory/src/main.rs`
- `examples/store-and-kernel/src/main.rs`

**Step 7: Run full test suite**

Run: `cargo test --all`
Expected: All tests pass.

**Step 8: Commit**

```
feat(kernel): rename apply_event to apply and apply_events to apply_all
```

---

### Task 2: Add `replay(version, &event)` to `AggregateRoot` and remove `load_from_events`

**Files:**
- Modify: `crates/nexus/src/aggregate.rs` (add `replay`, remove `load_from_events`)
- Create: `crates/nexus/tests/kernel_tests/replay_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/mod.rs` (add module)

**Step 1: Write failing tests for `replay`**

Create `crates/nexus/tests/kernel_tests/replay_tests.rs` using the existing test domain types
(`ItemAggregate`, `ItemEvent`, `TestId`, etc.) from the test helpers. Tests should cover:

```rust
// Uses existing test domain types from the kernel test suite

#[test]
fn replay_single_event_advances_version() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    root.replay(Version::from_persisted(1), &ItemEvent::Created(ItemCreated { name: "test".into() }))
        .unwrap();
    assert_eq!(root.version(), Version::from_persisted(1));
    assert_eq!(root.state().name, "test");
}

#[test]
fn replay_sequential_events() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    root.replay(Version::from_persisted(1), &ItemEvent::Created(ItemCreated { name: "a".into() })).unwrap();
    root.replay(Version::from_persisted(2), &ItemEvent::Done(ItemDone)).unwrap();
    assert_eq!(root.version(), Version::from_persisted(2));
    assert!(root.state().done);
}

#[test]
fn replay_rejects_wrong_version() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    let err = root.replay(Version::from_persisted(5), &ItemEvent::Created(ItemCreated { name: "a".into() }))
        .unwrap_err();
    assert!(matches!(err, KernelError::VersionMismatch { .. }));
}

#[test]
fn replay_rejects_duplicate_version() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    root.replay(Version::from_persisted(1), &ItemEvent::Created(ItemCreated { name: "a".into() })).unwrap();
    let err = root.replay(Version::from_persisted(1), &ItemEvent::Done(ItemDone)).unwrap_err();
    assert!(matches!(err, KernelError::VersionMismatch { .. }));
}

#[test]
fn replay_then_apply_continues_from_correct_version() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    root.replay(Version::from_persisted(1), &ItemEvent::Created(ItemCreated { name: "a".into() })).unwrap();
    // Now apply a NEW event — should be version 2
    root.apply(ItemEvent::Done(ItemDone));
    assert_eq!(root.current_version(), Version::from_persisted(2));
    // The replayed event is persisted, the applied event is uncommitted
    assert_eq!(root.version(), Version::from_persisted(1));
    let uncommitted = root.take_uncommitted_events();
    assert_eq!(uncommitted.len(), 1);
}

#[test]
fn replay_does_not_record_uncommitted() {
    let mut root = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    root.replay(Version::from_persisted(1), &ItemEvent::Created(ItemCreated { name: "a".into() })).unwrap();
    let events = root.take_uncommitted_events();
    assert!(events.is_empty());
    // version should not change after take since there were no uncommitted events
    assert_eq!(root.version(), Version::from_persisted(1));
}

#[test]
fn replay_enforces_rehydration_limit() {
    // Use a custom aggregate with MAX_REHYDRATION_EVENTS = 3
    // to test the limit without needing millions of events
    // (define a SmallLimitAggregate with MAX_REHYDRATION_EVENTS = 3)
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus replay`
Expected: FAIL — `replay` method does not exist.

**Step 3: Implement `replay` on `AggregateRoot`**

In `crates/nexus/src/aggregate.rs`, add to `impl<A: Aggregate> AggregateRoot<A>`:

```rust
/// Replay a single persisted event during streaming rehydration.
///
/// Validates that the version is strictly sequential, applies the event
/// to state, and advances the persisted version. Does NOT record the
/// event as uncommitted — this is for rehydration, not command handling.
///
/// Takes `&EventOf<A>` (not owned) to support zero-copy codecs where
/// the decoded event borrows from the payload buffer.
///
/// # Errors
///
/// Returns [`KernelError::VersionMismatch`] if `version` is not exactly
/// `self.version().next()`.
///
/// Returns [`KernelError::RehydrationLimitExceeded`] if the version
/// exceeds `Aggregate::MAX_REHYDRATION_EVENTS`.
pub fn replay(
    &mut self,
    version: Version,
    event: &EventOf<A>,
) -> Result<(), KernelError> {
    let expected = self.version.next();
    if version != expected {
        return Err(KernelError::VersionMismatch {
            stream_id: ErrorId::from_display(&self.id),
            expected,
            actual: version,
        });
    }
    if version.as_u64() > u64::try_from(A::MAX_REHYDRATION_EVENTS).unwrap_or(u64::MAX) {
        return Err(KernelError::RehydrationLimitExceeded {
            stream_id: ErrorId::from_display(&self.id),
            max: A::MAX_REHYDRATION_EVENTS,
        });
    }
    self.state.apply(event);
    self.version = version;
    Ok(())
}
```

**Step 4: Run replay tests to verify they pass**

Run: `cargo test -p nexus replay`
Expected: All PASS.

**Step 5: Remove `load_from_events` from `AggregateRoot`**

Delete the `load_from_events` method from `crates/nexus/src/aggregate.rs`.
Also remove it from `AggregateEntity` trait if present (check — it is currently only on `AggregateRoot`).

**Step 6: Run `cargo test --all 2>&1 | head -80` to find all broken call sites**

Expected: Compile errors in every file that called `load_from_events`.

**Step 7: Migrate all `load_from_events` call sites to `replay` loops**

Pattern — replace:
```rust
let agg = AggregateRoot::<A>::load_from_events(id, events)?;
```
With:
```rust
let mut agg = AggregateRoot::<A>::new(id);
for ve in events {
    let (version, event) = ve.into_parts();
    agg.replay(version, &event)?;
}
```

Or for the macro-generated newtypes (`BankAccount::load_from_events`), check if
the macro generates this method. If yes, update the macro too.

Files to migrate:
- `crates/nexus/tests/kernel_tests/aggregate_root_tests.rs`
- `crates/nexus/tests/kernel_tests/edge_case_tests.rs`
- `crates/nexus/tests/kernel_tests/security_tests.rs`
- `crates/nexus/tests/kernel_tests/property_tests.rs`
- `crates/nexus/tests/kernel_tests/integration_test.rs`
- `crates/nexus/tests/kernel_tests/newtype_aggregate_tests.rs`
- `crates/nexus-macros/tests/aggregate_derive.rs`
- `crates/nexus-macros/tests/macro_property_tests.rs`
- `crates/nexus-macros/tests/expand_roundtrip.rs`
- `crates/nexus-macros/tests/cross_crate_test/src/lib.rs`
- `examples/inmemory/src/main.rs`
- `examples/store-and-kernel/src/main.rs`

**Important:** Check the `#[nexus::aggregate]` proc macro in `crates/nexus-macros/src/`.
If it generates `load_from_events`, it needs updating. Search for `load_from_events`
in the macro source code.

**Step 8: Run full test suite**

Run: `cargo test --all`
Expected: All tests pass.

**Step 9: Commit**

```
feat(kernel)!: add replay() for streaming rehydration, remove load_from_events

BREAKING CHANGE: load_from_events removed. Use AggregateRoot::new() + replay() loop.
replay() takes &event for zero-copy codec compatibility on IoT/mobile targets.
```

---

### Task 3: Add `schema_version` to `PersistedEnvelope`

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs` (add field, update constructor)
- Create: `crates/nexus-store/tests/schema_version_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/schema_version_tests.rs`:

```rust
use nexus::Version;
use nexus_store::PersistedEnvelope;

#[test]
fn persisted_envelope_stores_schema_version() {
    let env = PersistedEnvelope::new("stream-1", Version::from_persisted(1), "UserCreated", 1, b"payload", ());
    assert_eq!(env.schema_version(), 1);
}

#[test]
fn persisted_envelope_schema_version_max() {
    let env = PersistedEnvelope::new("s", Version::from_persisted(1), "E", u32::MAX, b"p", ());
    assert_eq!(env.schema_version(), u32::MAX);
}

#[test]
#[should_panic(expected = "schema_version must be > 0")]
fn persisted_envelope_rejects_zero_schema_version() {
    PersistedEnvelope::new("s", Version::from_persisted(1), "E", 0, b"p", ());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store schema_version`
Expected: FAIL — constructor signature doesn't accept `schema_version`.

**Step 3: Add `schema_version` field to `PersistedEnvelope`**

In `crates/nexus-store/src/envelope.rs`:

Add `schema_version: u32` field to the struct.

Update `new()`:
```rust
pub const fn new(
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
) -> Self {
    assert!(schema_version > 0, "schema_version must be > 0: a schema version of 0 is invalid");
    Self { stream_id, version, event_type, schema_version, payload, metadata }
}
```

Add accessor:
```rust
#[must_use]
pub const fn schema_version(&self) -> u32 {
    self.schema_version
}
```

Update `pub use` in `crates/nexus-store/src/lib.rs` if needed.

**Step 4: Run schema_version tests**

Run: `cargo test -p nexus-store schema_version`
Expected: All PASS.

**Step 5: Fix all existing `PersistedEnvelope::new()` call sites**

Add `schema_version: 1` parameter to all existing calls. Files:
- `crates/nexus-store/tests/integration_tests.rs`
- `crates/nexus-store/tests/envelope_tests.rs`
- `crates/nexus-store/tests/raw_store_tests.rs`
- `crates/nexus-store/tests/stream_tests.rs`
- `crates/nexus-store/tests/bug_hunt_tests.rs`
- `crates/nexus-store/tests/security_tests.rs`
- `crates/nexus-store/tests/property_tests.rs`
- `examples/store-and-kernel/src/main.rs`
- `examples/store-inmemory/src/main.rs`

**Step 6: Run full store tests**

Run: `cargo test -p nexus-store`
Expected: All tests pass.

**Step 7: Run benchmarks to check nothing broke**

Run: `cargo bench -p nexus-store`

**Step 8: Commit**

```
feat(store)!: add schema_version to PersistedEnvelope for upcaster support

BREAKING CHANGE: PersistedEnvelope::new() now requires schema_version: u32 parameter.
Panics on 0 — schema versions start at 1. Only on read path (PersistedEnvelope),
writes (PendingEnvelope) are always current schema.
```

---

### Task 4: Add `InMemoryStore` behind `testing` feature

**Files:**
- Create: `crates/nexus-store/src/testing.rs`
- Modify: `crates/nexus-store/src/lib.rs` (add `testing` module)
- Modify: `crates/nexus-store/Cargo.toml` (add `testing` feature, add `tokio` dep behind feature)

**Step 1: Add feature flag and dependency**

In `crates/nexus-store/Cargo.toml`, add:
```toml
[features]
testing = ["dep:tokio"]

[dependencies]
tokio = { workspace = true, features = ["sync"], optional = true }
```

In `crates/nexus-store/src/lib.rs`, add:
```rust
#[cfg(feature = "testing")]
pub mod testing;
```

**Step 2: Write `InMemoryStore` tests first**

Create test in `crates/nexus-store/tests/inmemory_store_tests.rs`:

```rust
//! Tests for the provided InMemoryStore.
//! These validate that the testing::InMemoryStore correctly
//! implements RawEventStore and EventStream contracts.

use nexus::Version;
use nexus_store::testing::InMemoryStore;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::pending_envelope;

#[tokio::test]
async fn append_and_read_back() { ... }

#[tokio::test]
async fn optimistic_concurrency_rejects_wrong_version() { ... }

#[tokio::test]
async fn read_empty_stream() { ... }

#[tokio::test]
async fn read_from_version_filters_correctly() { ... }

#[tokio::test]
async fn sequential_version_validation_on_append() { ... }

#[tokio::test]
async fn schema_version_stored_and_returned() { ... }
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p nexus-store --features testing inmemory`
Expected: FAIL — `testing` module doesn't exist yet.

**Step 4: Implement `InMemoryStore`**

Create `crates/nexus-store/src/testing.rs`:

```rust
//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::raw::RawEventStore;
use crate::stream::EventStream;
use nexus::Version;
use std::collections::HashMap;
use tokio::sync::Mutex;

struct StoredRow {
    version: u64,
    event_type: String,
    schema_version: u32,
    payload: Vec<u8>,
}

pub struct InMemoryStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

// impl RawEventStore for InMemoryStore { ... }
// struct InMemoryStream { ... }
// impl EventStream for InMemoryStream { ... }
```

Key implementation details:
- Optimistic concurrency: check `stream.len() == expected_version.as_u64()`
- Sequential version validation on append: each envelope version must be `expected_version + 1 + i`
- `schema_version` defaults to 1 for events appended via `PendingEnvelope` (which doesn't carry schema_version)
- `InMemoryStream` copies rows out of the lock (same pattern as examples) to enable lending

**Step 5: Run InMemoryStore tests**

Run: `cargo test -p nexus-store --features testing inmemory`
Expected: All PASS.

**Step 6: Update dev-dependencies for nexus-store's own tests**

In `crates/nexus-store/Cargo.toml`, ensure dev-dependencies enable the feature:
```toml
[dev-dependencies]
nexus-store = { path = ".", features = ["testing"] }
```

Or alternatively, enable the feature in test code via `#[cfg(feature = "testing")]` on the test file.

**Step 7: Run full test suite**

Run: `cargo test --all`
Expected: All pass.

**Step 8: Run `cargo hakari generate --diff` and `cargo hakari verify`**

The new optional `tokio` dependency may need workspace-hack updates.

**Step 9: Commit**

```
feat(store): add InMemoryStore behind testing feature flag

Provides a ready-made RawEventStore + EventStream for tests and examples.
Includes optimistic concurrency, sequential version validation, and
schema_version tracking. Gated behind features = ["testing"].
```

---

### Task 5: Add `Repository<A>` trait

**Files:**
- Create: `crates/nexus-store/src/repository.rs`
- Modify: `crates/nexus-store/src/lib.rs` (add module, re-export)

**Step 1: Create the trait**

Create `crates/nexus-store/src/repository.rs`:

```rust
use crate::error::StoreError;
use nexus::aggregate::{Aggregate, AggregateRoot};
use std::future::Future;

/// Port for loading and saving aggregates via event streams.
///
/// Implementations handle codec encode/decode, streaming rehydration
/// via `AggregateRoot::replay()`, and version tracking internally.
/// Users interact with aggregates, not envelopes.
pub trait Repository<A: Aggregate>: Send + Sync {
    /// The error type for repository operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load an aggregate by replaying its event stream.
    ///
    /// Streams events from the store one-by-one through `replay()`,
    /// enabling zero-allocation rehydration with zero-copy codecs.
    /// Returns a fresh aggregate at `Version::INITIAL` if the stream is empty.
    fn load(
        &self,
        stream_id: &str,
        id: A::Id,
    ) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    /// Persist uncommitted events from an aggregate.
    ///
    /// Drains uncommitted events via `take_uncommitted_events()`,
    /// encodes them, and appends to the store with optimistic concurrency.
    /// The aggregate's persisted version advances on success.
    fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

**Step 2: Add module to lib.rs**

In `crates/nexus-store/src/lib.rs`:
```rust
pub mod repository;
pub use repository::Repository;
```

**Step 3: Verify it compiles**

Run: `cargo check -p nexus-store`
Expected: Compiles cleanly.

**Step 4: Commit**

```
feat(store): add Repository<A> trait for aggregate load/save

Trait-based port for streaming rehydration and event persistence.
Takes &event references for zero-copy codec compatibility.
Implementations wire RawEventStore + Codec + AggregateRoot::replay() internally.
```

---

### Task 6: Migrate existing tests to use `InMemoryStore` and update examples

**Files:**
- Modify: `crates/nexus-store/tests/raw_store_tests.rs` (replace custom store)
- Modify: `crates/nexus-store/tests/integration_tests.rs` (replace custom store)
- Modify: `crates/nexus-store/tests/property_tests.rs` (replace custom store)
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs` (keep ProbeStore — it has special behavior)
- Modify: `crates/nexus-store/tests/security_tests.rs` (replace custom store)
- Modify: `examples/store-inmemory/src/main.rs` (use InMemoryStore)
- Modify: `examples/store-and-kernel/src/main.rs` (use InMemoryStore + Repository pattern)

**Step 1: Migrate store test files to InMemoryStore**

For each test file that defines its own `InMemoryRawStore` / `TestStore`:
- Remove the local store struct and EventStream impl
- Import `nexus_store::testing::InMemoryStore`
- Update test code to use `InMemoryStore::new()`
- Keep `ProbeStore` in `bug_hunt_tests.rs` — it has intentional probing behavior

**Step 2: Update examples**

`store-inmemory` example: Replace `VecStore`/`VecStream` with `InMemoryStore`.
Keep the `JsonCodec`, `RenameUpcaster`, and `TodoEvent` — those demonstrate user code.

`store-and-kernel` example: Replace `InMemStore`/`InMemStream` with `InMemoryStore`.
Rewrite the bridge functions to show the `Repository` pattern (or keep as manual
demonstration of what the Repository does internally).

**Step 3: Update example Cargo.toml files**

Add `features = ["testing"]` to nexus-store dependency in both example crates.

**Step 4: Run all tests and examples**

Run: `cargo test --all && cargo run -p nexus-example-store-inmemory && cargo run -p nexus-example-store-and-kernel`
Expected: All pass/run correctly.

**Step 5: Commit**

```
refactor(store): migrate tests and examples to InMemoryStore

Removes 5 duplicate in-memory store implementations. Tests and examples
now use nexus_store::testing::InMemoryStore. ProbeStore in bug_hunt_tests
kept for its specialized probing behavior.
```

---

### Task 7: Final verification and cleanup

**Step 1: Run clippy with deny warnings**

Run: `cargo clippy --all-targets -- --deny warnings`
Expected: No warnings.

**Step 2: Run formatting check**

Run: `cargo fmt --all --check`

**Step 3: Run taplo**

Run: `taplo fmt --check`

**Step 4: Run hakari verification**

Run: `cargo hakari generate --diff && cargo hakari verify`

**Step 5: Run full test suite one final time**

Run: `cargo test --all`

**Step 6: Commit any remaining fixes**

```
chore: final cleanup for store DX improvements
```
