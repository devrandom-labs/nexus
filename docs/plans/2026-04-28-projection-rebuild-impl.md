# Projection Rebuilding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Auto-detect stale projection state via schema version mismatch and replay from the beginning of the stream — no operator action required.

**Architecture:** Add `persists_state()` to `StatePersistence` trait. Runner startup detects checkpoint-exists + no-usable-state + persistence-enabled → subscribes from `None` instead of last checkpoint. No new traits, no `delete` methods, stale data overwritten naturally.

**Tech Stack:** Rust, tokio, nexus-framework projection runner

---

### Task 1: Add `persists_state()` to `StatePersistence` trait

**Files:**
- Modify: `crates/nexus-framework/src/projection/persist.rs:18-37` (trait definition)
- Modify: `crates/nexus-framework/src/projection/persist.rs:49-59` (`NoStatePersistence` impl)
- Test: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Write the failing tests**

Add to `tests/projection_runner_tests.rs` in the StatePersistence section (after line 795):

```rust
#[test]
fn no_state_persistence_does_not_persist_state() {
    assert!(!NoStatePersistence.persists_state());
}

#[test]
fn with_state_persistence_persists_state() {
    let store = InMemoryStateStore::new();
    let sp = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::MIN);
    assert!(sp.persists_state());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-framework --features testing -- persists_state`
Expected: FAIL — `persists_state` method not found

**Step 3: Add `persists_state()` to the trait and impls**

In `crates/nexus-framework/src/projection/persist.rs`, add to the `StatePersistence<S>` trait (after `save`):

```rust
    /// Whether this persistence layer actually stores state.
    ///
    /// Returns `false` for [`NoStatePersistence`]. The projection runner
    /// uses this to distinguish "no state because first run" from "no state
    /// because schema version changed" — when `true` and `load` returns
    /// `None` despite a checkpoint existing, the runner replays from the
    /// beginning of the stream.
    fn persists_state(&self) -> bool {
        true
    }
```

Override in `NoStatePersistence` impl:

```rust
    fn persists_state(&self) -> bool {
        false
    }
```

`WithStatePersistence` keeps the default `true`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-framework --features testing -- persists_state`
Expected: PASS

**Step 5: Run full check**

Run: `nix flake check`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/nexus-framework/src/projection/persist.rs crates/nexus-framework/tests/projection_runner_tests.rs
git commit -m "feat(framework): add persists_state() to StatePersistence trait"
```

---

### Task 2: Schema-mismatch rebuild in runner startup

**Files:**
- Modify: `crates/nexus-framework/src/projection/runner.rs:69-92` (startup logic)
- Test: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Write the failing test — schema bump triggers rebuild from beginning**

Add to the Sequence/Protocol section of `tests/projection_runner_tests.rs`:

```rust
#[tokio::test]
async fn runner_rebuilds_from_beginning_on_schema_version_bump() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    // Append 3 events, run with schema v1
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: checkpoint at 3, state saved with schema v1
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));

    // Now restart with schema v2 — should rebuild from beginning
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner2
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should reflect ALL 3 events from initial() — not resume from v3
    let persisted = state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );

    // Checkpoint should be updated to v3
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-framework --features testing -- runner_rebuilds_from_beginning_on_schema_version_bump`
Expected: FAIL — the runner resumes from checkpoint v3 and finds no new events, so state is `initial()` with count:0, total:0.

**Step 3: Implement the runner startup change**

In `crates/nexus-framework/src/projection/runner.rs`, replace lines 69-92 with:

```rust
        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let loaded_state = state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?;

        // Detect schema mismatch: state persistence is enabled, a checkpoint
        // exists (prior progress), but no usable state was loaded (schema
        // version changed). In this case, replay from the beginning of the
        // stream to rebuild the projection with the new schema.
        let (mut state, resume_from) = match loaded_state {
            Some((s, _)) => (s, last_checkpoint),
            None if state_persistence.persists_state() && last_checkpoint.is_some() => {
                (projector.initial(), None)
            }
            None => (projector.initial(), last_checkpoint),
        };

        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut last_persisted_version = resume_from;
        let mut current_version = resume_from;
        let mut dirty = false;
```

Key change: `last_persisted_version` and `current_version` are initialized from `resume_from` (which is `None` during rebuild), not `last_checkpoint`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-framework --features testing -- runner_rebuilds_from_beginning_on_schema_version_bump`
Expected: PASS

**Step 5: Run full check**

Run: `nix flake check`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/nexus-framework/src/projection/runner.rs crates/nexus-framework/tests/projection_runner_tests.rs
git commit -m "feat(framework): auto-rebuild projection on schema version mismatch (#158)"
```

---

### Task 3: Rebuild completes, then normal resume on next restart

**Files:**
- Test: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Write the test — rebuild + new events + restart = normal resume**

Add to the Sequence/Protocol section:

```rust
#[tokio::test]
async fn runner_resumes_normally_after_rebuild_completes() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    // Phase 1: initial run with schema v1
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Phase 2: restart with schema v2 — triggers rebuild
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner2
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Phase 3: append more events, restart with same schema v2 — normal resume
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(30)],
    )
    .await;

    let runner3 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner3
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: state reflects all 3 events, checkpoint at v3
    let persisted = state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );

    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test -p nexus-framework --features testing -- runner_resumes_normally_after_rebuild_completes`
Expected: PASS (implementation from Task 2 should handle this)

**Step 3: Commit**

```bash
git add crates/nexus-framework/tests/projection_runner_tests.rs
git commit -m "test(framework): verify normal resume after rebuild completes"
```

---

### Task 4: Lifecycle test — crash mid-rebuild is idempotent

**Files:**
- Test: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Write the test — interrupt rebuild before trigger, restart rebuilds again**

Add to the Lifecycle section:

```rust
#[tokio::test]
async fn runner_rebuild_is_idempotent_after_crash_before_trigger() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    // Phase 1: initial run with schema v1
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint at v3 with schema v1
    assert_eq!(
        store.load(&stream_id).await.unwrap(),
        Some(Version::new(3).unwrap())
    );

    // Phase 2: start rebuild with schema v2 but trigger every 100 events
    // so the trigger never fires during 3 events. Immediate shutdown
    // simulates a crash before any checkpoint/state is persisted.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tx.send(()).unwrap();

    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .trigger(ProjEveryNEvents(NonZeroU64::new(100).unwrap()))
        .build();

    runner2
        .run(async {
            rx.await.ok();
        })
        .await
        .unwrap();

    // Checkpoint still at v3 (from schema v1 run), no v2 state saved
    // because shutdown happened before any events were processed
    assert_eq!(
        store.load(&stream_id).await.unwrap(),
        Some(Version::new(3).unwrap())
    );
    assert!(state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .is_none());

    // Phase 3: restart with schema v2 — should detect mismatch again
    // and rebuild from beginning (idempotent)
    let runner3 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner3
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Now state should be correctly rebuilt
    let persisted = state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test -p nexus-framework --features testing -- runner_rebuild_is_idempotent_after_crash_before_trigger`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-framework/tests/projection_runner_tests.rs
git commit -m "test(framework): verify rebuild idempotency after crash mid-rebuild"
```

---

### Task 5: Defensive boundary tests

**Files:**
- Test: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Write test — `NoStatePersistence` with checkpoint does NOT trigger rebuild**

Add to the Defensive Boundary section:

```rust
#[tokio::test]
async fn runner_no_state_persistence_with_checkpoint_does_not_rebuild() {
    let store = InMemoryStore::new();
    let stream_id = TestId("stream-1".into());

    // Append 3 events, run WITHOUT state persistence to set a checkpoint
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint at v3, no state persisted (NoStatePersistence)
    assert_eq!(
        store.load(&stream_id).await.unwrap(),
        Some(Version::new(3).unwrap())
    );

    // Append 2 more events
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(40), TestEvent::Added(50)],
    )
    .await;

    // Run again without state persistence — should resume from v3, NOT rebuild
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    runner2
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint should be at v5 (resumed from v3, processed v4+v5)
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(5).unwrap()));
}
```

**Step 2: Write test — first run with state persistence, no checkpoint, no state = normal start**

```rust
#[tokio::test]
async fn runner_first_run_with_state_persistence_is_not_rebuild() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // First run ever — no checkpoint, no state. Should process from beginning
    // without treating it as a "rebuild".
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );
}
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p nexus-framework --features testing -- no_state_persistence_with_checkpoint_does_not_rebuild`
Run: `cargo test -p nexus-framework --features testing -- first_run_with_state_persistence_is_not_rebuild`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/nexus-framework/tests/projection_runner_tests.rs
git commit -m "test(framework): defensive boundary tests for rebuild detection"
```

---

### Task 6: Update runner doc comment + final check

**Files:**
- Modify: `crates/nexus-framework/src/projection/runner.rs:41-53` (doc comment on `run`)

**Step 1: Update the doc comment on `run()`**

Replace the event loop doc to document the rebuild behavior:

```rust
    /// Run the projection loop until shutdown or error.
    ///
    /// # Startup
    ///
    /// 1. Load checkpoint (resume position)
    /// 2. Load persisted state
    /// 3. **Schema mismatch detection:** if state persistence is enabled,
    ///    a checkpoint exists, but no usable state was loaded (schema version
    ///    changed), the runner replays from the beginning of the stream
    ///    to rebuild the projection with the new schema.
    ///
    /// # Event loop
    ///
    /// 1. Subscribe to the event stream from the resume position
    /// 2. For each event: decode -> apply -> trigger check -> maybe persist + checkpoint
    /// 3. On shutdown: flush dirty state + checkpoint, return `Ok(())`
    ///
    /// # Errors
    ///
    /// Returns immediately on any error. The supervision layer (separate
    /// component) is responsible for retry/restart policy.
```

**Step 2: Run full check**

Run: `nix flake check`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-framework/src/projection/runner.rs
git commit -m "docs(framework): document schema-mismatch rebuild in runner"
```

---

### Task 7: Update CLAUDE.md architecture section

**Files:**
- Modify: `CLAUDE.md` (Framework crate → projection → runner description)

**Step 1: Update the runner description in CLAUDE.md**

In the `nexus-framework` architecture section, update the `runner.rs` bullet to mention schema-mismatch rebuild:

Add after "Uses `tokio::select!` to race event stream against shutdown signal.":

> Auto-detects schema version mismatch at startup (checkpoint exists but state is stale) and replays from the beginning of the stream.

**Step 2: Run full check**

Run: `nix flake check`
Expected: PASS

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document projection rebuild in architecture section"
```
