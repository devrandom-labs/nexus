# Phased Startup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split `ProjectionRunner::run()` into `initialize() -> Initialized -> run(shutdown)` with three-typestate compile-time safety.

**Architecture:** New files `prepared.rs` (PreparedProjection + markers + event loop) and `initialized.rs` (Initialized enum) alongside existing `runner.rs` (now holds only ProjectionRunner + initialize + resolve_startup). The event loop moves from runner to PreparedProjection unchanged. Three typestate markers (Resuming, Rebuilding, Starting) enforce supervisor control at compile time.

**Tech Stack:** Rust edition 2024, tokio, PhantomData typestates

**Design doc:** `docs/plans/2026-04-28-phased-startup-design.md`

---

### Task 1: Create type scaffolding

Create the new files with types only — no logic yet. This must compile.

**Files:**
- Create: `crates/nexus-framework/src/projection/prepared.rs`
- Create: `crates/nexus-framework/src/projection/initialized.rs`
- Modify: `crates/nexus-framework/src/projection/mod.rs`

**Step 1: Create `prepared.rs` with markers and PreparedProjection struct**

```rust
use std::marker::PhantomData;

use nexus::Version;
use nexus_store::projection::Projector;

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers — encode the startup decision at the type level
// ═══════════════════════════════════════════════════════════════════════════

/// Startup decision: resuming from a checkpoint with loaded state.
pub struct Resuming;

/// Startup decision: schema mismatch detected, replaying from beginning.
pub struct Rebuilding;

/// Startup decision: first run, processing from beginning.
pub struct Starting;

// ═══════════════════════════════════════════════════════════════════════════
// PreparedProjection
// ═══════════════════════════════════════════════════════════════════════════

/// A projection that has completed startup and is ready to run.
///
/// Created by [`ProjectionRunner::initialize`](super::ProjectionRunner).
/// The `Mode` parameter encodes the startup decision as a typestate:
/// - [`Resuming`] — loaded state, will resume from checkpoint
/// - [`Rebuilding`] — schema mismatch, will replay from beginning
/// - [`Starting`] — first run, will process from beginning
///
/// Call [`run`](PreparedProjection::run) to enter the event loop, or inspect
/// the resolved state via mode-specific accessors before running.
pub struct PreparedProjection<I, Sub, Ckpt, SP, P: Projector, EC, Trig, Mode> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) state: P::State,
    pub(crate) resume_from: Option<Version>,
    pub(crate) _mode: PhantomData<Mode>,
}
```

**Step 2: Create `initialized.rs` with the Initialized enum**

```rust
use nexus_store::projection::Projector;

use super::prepared::{PreparedProjection, Rebuilding, Resuming, Starting};

// ═══════════════════════════════════════════════════════════════════════════
// Initialized — the return type of ProjectionRunner::initialize()
// ═══════════════════════════════════════════════════════════════════════════

/// The result of [`ProjectionRunner::initialize`](super::ProjectionRunner).
///
/// Forces the supervisor to handle all three startup outcomes:
/// - [`Resuming`] — loaded state, can [`force_rebuild`](PreparedProjection::force_rebuild) or run
/// - [`Rebuilding`] — schema mismatch, can run or drop to abort
/// - [`Starting`] — first run, can run or drop to abort
pub enum Initialized<I, Sub, Ckpt, SP, P: Projector, EC, Trig> {
    /// Checkpoint and state loaded successfully. Will resume from checkpoint.
    Resuming(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>),
    /// Schema mismatch detected. Will replay from beginning of stream.
    Rebuilding(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>),
    /// First run. Will process from beginning of stream.
    Starting(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>),
}
```

**Step 3: Update `mod.rs` with new modules and re-exports**

```rust
mod builder;
mod error;
mod initialized;
mod persist;
mod prepared;
mod runner;
mod stream;

pub use builder::ProjectionRunnerBuilder;
pub use error::{ProjectionError, StatePersistError};
pub use initialized::Initialized;
pub use persist::{NoStatePersistence, StatePersistence, WithStatePersistence};
pub use prepared::{PreparedProjection, Rebuilding, Resuming, Starting};
pub use runner::ProjectionRunner;
```

**Step 4: Verify compilation**

Run: `cargo check -p nexus-framework`
Expected: compiles (types exist but have no methods yet; nothing references them except re-exports)

**Step 5: Commit**

```
feat(framework): scaffold PreparedProjection and Initialized types
```

---

### Task 2: Refactor resolve_startup to return StartupOutcome

Currently returns `(S, Option<Version>)`. Refactor to return a typed enum so `initialize()` can construct the correct `Initialized` variant.

**Files:**
- Modify: `crates/nexus-framework/src/projection/runner.rs`

**Step 1: Write failing tests — update existing resolve_startup tests to expect enum variants**

Replace all 7 existing `resolve_startup` tests. Each currently asserts on a tuple `(state, resume)` — change to assert on the `StartupOutcome` variant. Example for the first test:

```rust
#[test]
fn startup_resumes_from_checkpoint_when_state_loaded() {
    let outcome = resolve_startup(Some(("loaded", v(5))), Some(v(5)), true, || "initial");
    let StartupOutcome::Resuming { state, resume_from } = outcome else {
        panic!("expected Resuming");
    };
    assert_eq!(state, "loaded");
    assert_eq!(resume_from, Some(v(5)));
}

#[test]
fn startup_resumes_from_checkpoint_when_state_loaded_regardless_of_persists_flag() {
    let outcome = resolve_startup(Some(("loaded", v(3))), Some(v(3)), false, || "initial");
    let StartupOutcome::Resuming { state, resume_from } = outcome else {
        panic!("expected Resuming");
    };
    assert_eq!(state, "loaded");
    assert_eq!(resume_from, Some(v(3)));
}

#[test]
fn startup_rebuilds_when_persists_state_and_checkpoint_exists_but_no_state() {
    let outcome =
        resolve_startup(None::<(&str, Version)>, Some(v(10)), true, || "initial");
    let StartupOutcome::Rebuilding { state } = outcome else {
        panic!("expected Rebuilding");
    };
    assert_eq!(state, "initial");
}

#[test]
fn startup_first_run_with_persistence_enabled() {
    let outcome = resolve_startup(None::<(&str, Version)>, None, true, || "initial");
    let StartupOutcome::Starting { state } = outcome else {
        panic!("expected Starting");
    };
    assert_eq!(state, "initial");
}

#[test]
fn startup_resumes_from_checkpoint_without_state_persistence() {
    let outcome =
        resolve_startup(None::<(&str, Version)>, Some(v(7)), false, || "initial");
    let StartupOutcome::Resuming { state, resume_from } = outcome else {
        panic!("expected Resuming");
    };
    assert_eq!(state, "initial");
    assert_eq!(resume_from, Some(v(7)));
}

#[test]
fn startup_first_run_without_state_persistence() {
    let outcome = resolve_startup(None::<(&str, Version)>, None, false, || "initial");
    let StartupOutcome::Starting { state } = outcome else {
        panic!("expected Starting");
    };
    assert_eq!(state, "initial");
}

#[test]
fn startup_initial_is_lazy_when_state_loaded() {
    let mut called = false;
    let outcome = resolve_startup(Some(("loaded", v(1))), Some(v(1)), true, || {
        called = true;
        "initial"
    });
    assert!(
        matches!(outcome, StartupOutcome::Resuming { .. }),
        "expected Resuming"
    );
    assert!(
        !called,
        "initial() should not be called when state is loaded"
    );
}
```

Run: `cargo test -p nexus-framework -- startup`
Expected: FAIL — `StartupOutcome` does not exist yet

**Step 2: Add StartupOutcome enum and update resolve_startup**

Replace the existing `resolve_startup` function and add the enum above it:

```rust
/// Startup outcome from resolving checkpoint + persisted state.
///
/// Private to this module. Maps directly to the public [`Initialized`] variants.
enum StartupOutcome<S> {
    /// Resume from checkpoint with loaded or initial state.
    Resuming { state: S, resume_from: Option<Version> },
    /// Schema mismatch: replay from beginning with fresh state.
    Rebuilding { state: S },
    /// First run: process from beginning with initial state.
    Starting { state: S },
}

/// Resolve startup state from loaded checkpoint and persisted state.
///
/// # Decision table
///
/// | loaded_state | persists_state | checkpoint | outcome |
/// |---|---|---|---|
/// | `Some(s)` | any | any | `Resuming` |
/// | `None` | `true` | `Some(_)` | `Rebuilding` |
/// | `None` | `true` | `None` | `Starting` |
/// | `None` | `false` | `Some(_)` | `Resuming` |
/// | `None` | `false` | `None` | `Starting` |
fn resolve_startup<S>(
    loaded_state: Option<(S, Version)>,
    last_checkpoint: Option<Version>,
    persists_state: bool,
    initial: impl FnOnce() -> S,
) -> StartupOutcome<S> {
    match loaded_state {
        Some((state, _)) => StartupOutcome::Resuming {
            state,
            resume_from: last_checkpoint,
        },
        None if persists_state && last_checkpoint.is_some() => StartupOutcome::Rebuilding {
            state: initial(),
        },
        None if last_checkpoint.is_some() => StartupOutcome::Resuming {
            state: initial(),
            resume_from: last_checkpoint,
        },
        None => StartupOutcome::Starting {
            state: initial(),
        },
    }
}
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p nexus-framework -- startup`
Expected: all 7 resolve_startup tests pass

**Step 4: Commit**

```
refactor(framework): resolve_startup returns typed StartupOutcome enum
```

---

### Task 3: Move event loop to PreparedProjection::run()

Move `apply_event` and the event loop from `runner.rs` to `prepared.rs`. The loop is generic over `Mode` — all three typestates share the same event loop.

**Files:**
- Modify: `crates/nexus-framework/src/projection/prepared.rs`
- Modify: `crates/nexus-framework/src/projection/runner.rs`

**Step 1: Move `apply_event` to `prepared.rs`**

Cut `apply_event` and its imports from `runner.rs`. Paste into `prepared.rs` above the struct definition, in the "Sync core" section. Add required imports at the top of `prepared.rs`:

```rust
use std::iter;
use std::marker::PhantomData;

use nexus::{DomainEvent, Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::stream::{DecodeStreamError, DecodedStream};

// ═══════════════════════════════════════════════════════════════════════════
// Sync core — pure decision functions, no IO
// ═══════════════════════════════════════════════════════════════════════════

/// Process a single decoded event: fold through the projector, evaluate trigger.
///
/// Returns the new state and whether the trigger fired (indicating the
/// caller should persist state + checkpoint).
fn apply_event<P, E, Trig>(
    projector: &P,
    trigger: &Trig,
    state: P::State,
    event: &E,
    last_persisted_version: Option<Version>,
    version: Version,
) -> Result<(P::State, bool), P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    let event_name = event.name();
    let new_state = projector.apply(state, event)?;
    let should_persist =
        trigger.should_project(last_persisted_version, version, iter::once(event_name));
    Ok((new_state, should_persist))
}
```

**Step 2: Implement `run()` on PreparedProjection — generic over Mode**

Add this impl block to `prepared.rs` after the struct:

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Async shell — IO orchestration only
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, Ckpt, SP, P, EC, Trig, Mode> PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Mode>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Run the event loop until shutdown or error.
    ///
    /// 1. Subscribe to the event stream from the resolved resume position
    /// 2. For each event: decode -> apply -> trigger check -> maybe persist + checkpoint
    /// 3. On shutdown: flush dirty state + checkpoint, return `Ok(())`
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
            mut state,
            resume_from,
            _mode: _,
        } = self;

        // ── IO: subscribe ─────────────────────────────────────────────
        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut last_persisted_version = resume_from;
        let mut current_version = resume_from;
        let mut dirty = false;

        tokio::pin!(shutdown);

        loop {
            let item = tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            };

            let Some(result) = item else {
                // ── IO: flush on shutdown ──────────────────────────────
                if let (true, Some(ver)) = (dirty, current_version) {
                    state_persistence
                        .save(&id, ver, &state)
                        .await
                        .map_err(ProjectionError::State)?;
                    checkpoint
                        .save(&id, ver)
                        .await
                        .map_err(ProjectionError::Checkpoint)?;
                }
                return Ok(());
            };

            let (version, event) = match result {
                Ok(pair) => pair,
                Err(DecodeStreamError::Stream(e)) => {
                    return Err(ProjectionError::Subscription(e));
                }
                Err(DecodeStreamError::Codec(e)) => {
                    return Err(ProjectionError::EventCodec(e));
                }
            };

            // ── Sync: fold + trigger ──────────────────────────────────
            let (new_state, should_persist) = apply_event(
                &projector,
                &trigger,
                state,
                &event,
                last_persisted_version,
                version,
            )
            .map_err(ProjectionError::Projector)?;

            state = new_state;
            current_version = Some(version);
            dirty = true;

            // ── IO: persist if triggered ──────────────────────────────
            if should_persist {
                state_persistence
                    .save(&id, version, &state)
                    .await
                    .map_err(ProjectionError::State)?;
                checkpoint
                    .save(&id, version)
                    .await
                    .map_err(ProjectionError::Checkpoint)?;
                last_persisted_version = Some(version);
                dirty = false;
            }
        }
    }
}
```

**Step 3: Move apply_event tests from runner.rs to prepared.rs**

Cut the entire `apply_event` test section from `runner.rs` (tests: `apply_event_folds_state_through_projector`, `apply_event_returns_should_persist_true_when_trigger_fires`, `apply_event_returns_should_persist_false_when_trigger_does_not_fire`, `apply_event_propagates_projector_error`, `apply_event_passes_versions_to_trigger`). Paste into a `#[cfg(test)] mod tests` block at the bottom of `prepared.rs`. Include the shared test fixtures (`Evt`, `SumProjector`, `FailingProjector`, `AlwaysTrigger`, `NeverTrigger`, `v()`) — these are duplicated across both test modules since they're in `#[cfg(test)]` blocks.

**Step 4: Verify compilation and tests**

Run: `cargo check -p nexus-framework`
Expected: compiles — `runner.rs` still has the old `run()` method; `prepared.rs` has the new one

Run: `cargo test -p nexus-framework -- apply_event`
Expected: all 5 apply_event tests pass (now in prepared.rs)

**Step 5: Commit**

```
refactor(framework): move event loop to PreparedProjection::run()
```

---

### Task 4: Implement initialize() and remove old run()

Add `initialize()` to ProjectionRunner, remove old `run()`, update integration tests.

**Files:**
- Modify: `crates/nexus-framework/src/projection/runner.rs`
- Modify: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Implement `initialize()` on ProjectionRunner**

Replace the `run()` method in the existing impl block with:

```rust
use std::marker::PhantomData;

use super::initialized::Initialized;
use super::prepared::PreparedProjection;

impl<I, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Initialize the projection: load checkpoint, load state, resolve startup decision.
    ///
    /// Returns an [`Initialized`] enum that the supervisor must match:
    /// - [`Initialized::Starting`] — first run, will process from beginning
    /// - [`Initialized::Resuming`] — loaded state, will resume from checkpoint
    /// - [`Initialized::Rebuilding`] — schema mismatch, will replay from beginning
    ///
    /// Call `.run(shutdown)` on any variant to enter the event loop, or
    /// use [`Initialized::run`] as a convenience that delegates to whichever variant.
    pub async fn initialize(
        self,
    ) -> Result<
        Initialized<I, Sub, Ckpt, SP, P, EC, Trig>,
        ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>,
    > {
        let Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
        } = self;

        // ── IO: load ──────────────────────────────────────────────────
        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let loaded_state = state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?;

        // ── Sync: decide startup ──────────────────────────────────────
        let outcome = resolve_startup(
            loaded_state,
            last_checkpoint,
            state_persistence.persists_state(),
            || projector.initial(),
        );

        // ── Construct the appropriate typestate variant ────────────────
        match outcome {
            StartupOutcome::Resuming { state, resume_from } => {
                Ok(Initialized::Resuming(PreparedProjection {
                    id,
                    subscription,
                    checkpoint,
                    state_persistence,
                    projector,
                    event_codec,
                    trigger,
                    state,
                    resume_from,
                    _mode: PhantomData,
                }))
            }
            StartupOutcome::Rebuilding { state } => {
                Ok(Initialized::Rebuilding(PreparedProjection {
                    id,
                    subscription,
                    checkpoint,
                    state_persistence,
                    projector,
                    event_codec,
                    trigger,
                    state,
                    resume_from: None,
                    _mode: PhantomData,
                }))
            }
            StartupOutcome::Starting { state } => {
                Ok(Initialized::Starting(PreparedProjection {
                    id,
                    subscription,
                    checkpoint,
                    state_persistence,
                    projector,
                    event_codec,
                    trigger,
                    state,
                    resume_from: None,
                    _mode: PhantomData,
                }))
            }
        }
    }
}
```

Remove the old `run()` method entirely. Also remove imports that were only used by `run()` (they've moved to `prepared.rs`): `iter`, `tokio_stream::StreamExt`, `super::stream::{DecodeStreamError, DecodedStream}`.

The remaining imports in runner.rs should be:

```rust
use std::marker::PhantomData;

use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};

use super::error::ProjectionError;
use super::initialized::Initialized;
use super::persist::StatePersistence;
use super::prepared::PreparedProjection;
```

**Step 2: Update all integration tests to use the new API**

In `crates/nexus-framework/tests/projection_runner_tests.rs`, update the import line to include the new types:

```rust
use nexus_framework::projection::{
    Initialized, NoStatePersistence, PreparedProjection, ProjectionError, ProjectionRunner,
    Rebuilding, Resuming, Starting, StatePersistence, WithStatePersistence,
};
```

Then perform a global replacement in the test file: every occurrence of `runner.run(shutdown).await` becomes `runner.initialize().await.unwrap().run(shutdown).await`. Similarly for `runner.run(async { ... }).await` patterns.

Specifically, every test that does:

```rust
runner.run(shutdown).await.unwrap();
```

becomes:

```rust
runner.initialize().await.unwrap().run(shutdown).await.unwrap();
```

And every test that captures the result:

```rust
let result = runner.run(async { ... }).await;
```

becomes:

```rust
let result = runner.initialize().await.unwrap().run(async { ... }).await;
```

This is a mechanical replacement across all test functions. There are ~13 runner invocations across the test file.

**Step 3: Verify compilation and all tests pass**

Run: `cargo test -p nexus-framework`
Expected: all tests pass (unit tests in runner.rs + prepared.rs, integration tests in projection_runner_tests.rs)

**Step 4: Commit**

```
feat(framework): replace run() with initialize() -> Initialized -> run()
```

---

### Task 5: Implement typestate-specific methods

Add accessors per mode, `force_rebuild` on Resuming, and `Initialized::run()` convenience.

**Files:**
- Modify: `crates/nexus-framework/src/projection/prepared.rs`
- Modify: `crates/nexus-framework/src/projection/initialized.rs`

**Step 1: Write failing tests for accessors and force_rebuild**

Add these tests to the `#[cfg(test)]` module in `prepared.rs`:

```rust
use std::marker::PhantomData;
use super::*;

fn make_resuming_prepared() -> PreparedProjection<
    &'static str, (), (), (), SumProjector, (), NeverTrigger, Resuming
> {
    PreparedProjection {
        id: "test",
        subscription: (),
        checkpoint: (),
        state_persistence: (),
        projector: SumProjector,
        event_codec: (),
        trigger: NeverTrigger,
        state: 42,
        resume_from: Some(v(5)),
        _mode: PhantomData,
    }
}

#[test]
fn resuming_state_returns_loaded_state() {
    let p = make_resuming_prepared();
    assert_eq!(*p.state(), 42);
}

#[test]
fn resuming_resume_from_returns_checkpoint() {
    let p = make_resuming_prepared();
    assert_eq!(p.resume_from(), Some(v(5)));
}

#[test]
fn force_rebuild_transitions_to_rebuilding_with_initial_state() {
    let p = make_resuming_prepared();
    let rebuilt = p.force_rebuild();
    // State reset to projector.initial() = 0
    assert_eq!(*rebuilt.state(), 0);
}
```

Run: `cargo test -p nexus-framework -- resuming`
Expected: FAIL — methods don't exist yet

**Step 2: Implement accessors on Resuming**

Add to `prepared.rs`:

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Typestate-specific methods
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>
{
    /// The resolved resume position from the checkpoint.
    #[must_use]
    pub fn resume_from(&self) -> Option<Version> {
        self.resume_from
    }

    /// The loaded projection state.
    #[must_use]
    pub fn state(&self) -> &P::State {
        &self.state
    }

    /// Discard loaded state and force a full rebuild from the beginning.
    ///
    /// Transitions the typestate from [`Resuming`] to [`Rebuilding`].
    /// The state is reset to [`Projector::initial()`] and the resume
    /// position is set to `None` (start of stream).
    #[must_use]
    pub fn force_rebuild(
        self,
    ) -> PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding> {
        let initial_state = self.projector.initial();
        PreparedProjection {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            state: initial_state,
            resume_from: None,
            _mode: PhantomData,
        }
    }
}
```

**Step 3: Implement accessors on Rebuilding and Starting**

```rust
impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>
{
    /// The initial projection state (always [`Projector::initial()`]).
    #[must_use]
    pub fn state(&self) -> &P::State {
        &self.state
    }
}

impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>
{
    /// The initial projection state (always [`Projector::initial()`]).
    #[must_use]
    pub fn state(&self) -> &P::State {
        &self.state
    }
}
```

**Step 4: Implement `Initialized::run()` convenience**

Add to `initialized.rs`:

```rust
use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};

use super::error::ProjectionError;
use super::persist::StatePersistence;

impl<I, Sub, Ckpt, SP, P, EC, Trig> Initialized<I, Sub, Ckpt, SP, P, EC, Trig>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Run the event loop without inspecting the startup decision.
    ///
    /// Convenience method equivalent to matching all three variants and
    /// calling `.run(shutdown)` on each. Use when supervisor control
    /// is not needed.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        match self {
            Self::Resuming(p) => p.run(shutdown).await,
            Self::Rebuilding(p) => p.run(shutdown).await,
            Self::Starting(p) => p.run(shutdown).await,
        }
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p nexus-framework`
Expected: all tests pass, including the new accessor/force_rebuild tests

**Step 6: Commit**

```
feat(framework): typestate accessors, force_rebuild, and Initialized::run()
```

---

### Task 6: Add typestate behavior integration tests

Test the full initialize() → match → variant flow against the decision table. These are new integration tests that verify the phased API works end-to-end.

**Files:**
- Modify: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Add tests for initialize() returning correct variants**

Add a new section to the integration test file:

```rust
// ═══════════════════════════════════════════════════════════════════════════
// 7. Typestate / Phased Startup Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn initialize_returns_starting_on_first_run() {
    let store = InMemoryStore::new();
    let stream_id = TestId("fresh-stream".into());

    let runner = ProjectionRunner::builder(stream_id)
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    let initialized = runner.initialize().await.unwrap();
    assert!(matches!(initialized, Initialized::Starting(_)));
}

#[tokio::test]
async fn initialize_returns_resuming_after_successful_run() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run
    ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — should resume
    let runner2 = ProjectionRunner::builder(stream_id)
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    let initialized = runner2.initialize().await.unwrap();
    assert!(matches!(initialized, Initialized::Resuming(_)));
}

#[tokio::test]
async fn initialize_returns_rebuilding_on_schema_mismatch() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run with schema v1
    ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run with schema v2 — should trigger rebuild
    let runner2 = ProjectionRunner::builder(stream_id)
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    let initialized = runner2.initialize().await.unwrap();
    assert!(matches!(initialized, Initialized::Rebuilding(_)));
}

#[tokio::test]
async fn initialize_returns_resuming_without_state_persistence() {
    let store = InMemoryStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run (checkpoint-only, no state persistence)
    ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — should resume (not rebuild) despite no state
    let runner2 = ProjectionRunner::builder(stream_id)
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    let initialized = runner2.initialize().await.unwrap();
    assert!(matches!(initialized, Initialized::Resuming(_)));
}

#[tokio::test]
async fn force_rebuild_replays_from_beginning() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // First run
    ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — force rebuild despite valid state
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    let initialized = runner2.initialize().await.unwrap();
    let Initialized::Resuming(prepared) = initialized else {
        panic!("expected Resuming");
    };

    // Force rebuild and run
    prepared
        .force_rebuild()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should be freshly computed from initial()
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

#[tokio::test]
async fn resuming_accessors_return_correct_values() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run to create checkpoint + state
    ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — inspect Resuming variant
    let runner2 = ProjectionRunner::builder(stream_id)
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    let Initialized::Resuming(prepared) = runner2.initialize().await.unwrap() else {
        panic!("expected Resuming");
    };

    assert_eq!(prepared.resume_from(), Some(Version::new(1).unwrap()));
    assert_eq!(
        *prepared.state(),
        CountState {
            count: 1,
            total: 10
        }
    );
}
```

**Step 2: Run all tests**

Run: `cargo test -p nexus-framework`
Expected: all tests pass

**Step 3: Commit**

```
test(framework): typestate behavior tests for phased startup
```

---

### Task 7: Final verification

**Step 1: Run full flake check**

Run: `nix flake check`
Expected: all checks pass (clippy, fmt, tests, taplo, audit, deny, hakari)

**Step 2: Fix any clippy/fmt issues**

Run: `cargo fmt --all` if needed.

**Step 3: Update CLAUDE.md architecture section**

Update the `ProjectionRunner` description in the `nexus-framework` architecture section to reflect the new phased API:

In the `runner.rs` bullet, change to:
> `runner.rs` — `ProjectionRunner<Id, Sub, Ckpt, SP, P, EC, Trig>`: background event processor. `initialize()` loads checkpoint + state, resolves schema mismatch, returns `Initialized` enum. Single-use: consumes `self`.

Add new bullets:
> `prepared.rs` — `PreparedProjection<Id, Sub, Ckpt, SP, P, EC, Trig, Mode>`: initialized projection ready to run. Three typestate markers: `Resuming` (can `force_rebuild` or `run`), `Rebuilding` (can `run` or drop), `Starting` (can `run` or drop). `run()` subscribes to the event stream and enters the event loop.
>
> `initialized.rs` — `Initialized<Id, Sub, Ckpt, SP, P, EC, Trig>`: enum returned by `initialize()`. Variants: `Resuming`, `Rebuilding`, `Starting`. Convenience `run()` delegates to whichever variant.

**Step 4: Commit**

```
docs: update CLAUDE.md for phased projection startup
```
