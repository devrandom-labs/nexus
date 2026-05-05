# Projection API Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `ProjectionRunner`/`Initialized`/`PreparedProjection` with `Projection<Configured>`/`Projection<Ready>` and make `resolve_startup` return `ProjectionStatus::Idle` directly.

**Architecture:** Single `Projection<Mode>` struct with two typestate phases. `Configured` is a unit marker. `Ready<S>` carries `ProjectionStatus<S>` + `StartupDecision`. All startup/run logic lives in `projection.rs`. Old files deleted.

**Tech Stack:** Rust, nexus-framework crate, typestate pattern

**Design doc:** `docs/plans/2026-04-30-projection-cleanup-design.md`

---

### Task 1: Create `projection.rs` with types and `resolve_startup` tests

**Files:**
- Create: `crates/nexus-framework/src/projection/projection.rs`

**Step 1: Write the types, stub methods, and tests**

Create `projection.rs` with:
- `Configured` unit struct
- `StartupDecision` enum (Fresh/Resume/Rebuild)
- `Ready<S>` struct with `status: ProjectionStatus<S>` and `decision: StartupDecision`
- `Projection<I, Sub, Ckpt, SP, P, EC, Trig, Mode>` struct
- `resolve_startup` function returning `(ProjectionStatus<S>, StartupDecision)`
- Tests for all 5 decision table rows + laziness test

```rust
use nexus::Version;
use nexus_store::projection::Projector;

use super::status::ProjectionStatus;

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers
// ═══════════════════════════════════════════════════════════════════════════

/// Configured but not yet loaded. Produced by [`ProjectionBuilder::build`](super::ProjectionBuilder::build).
pub struct Configured;

/// The startup decision label — why the projection resolved to its current state.
///
/// All three variants produce `ProjectionStatus::Idle`. The label is for
/// supervisor inspection only — it has no behavioral effect on `run()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupDecision {
    /// First run — no checkpoint, no persisted state.
    Fresh,
    /// Loaded checkpoint and/or state. Resuming from where we left off.
    Resume,
    /// Schema mismatch — persisted state is stale, replaying from beginning.
    Rebuild,
}

/// Loaded and ready to run. Produced by [`Projection::initialize`].
pub struct Ready<S> {
    pub(crate) status: ProjectionStatus<S>,
    pub(crate) decision: StartupDecision,
}

// ═══════════════════════════════════════════════════════════════════════════
// Projection
// ═══════════════════════════════════════════════════════════════════════════

/// A subscription-powered async projection.
///
/// Two-phase lifecycle via typestate:
/// 1. `Projection<..., Configured>` — built, not yet loaded
/// 2. `Projection<..., Ready<P::State>>` — loaded, ready to run
///
/// Constructed via [`Projection::builder`]. Single-use: `run` consumes `self`.
pub struct Projection<I, Sub, Ckpt, SP, P: Projector, EC, Trig, Mode> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) mode: Mode,
}

// ═══════════════════════════════════════════════════════════════════════════
// Startup resolution
// ═══════════════════════════════════════════════════════════════════════════

/// Resolve startup state from loaded checkpoint and persisted state.
///
/// Returns `ProjectionStatus::Idle` directly — the startup decision only
/// determines what `state` and `checkpoint` values Idle starts with.
///
/// # Decision table
///
/// | loaded_state | persists_state | checkpoint | decision | Idle state |
/// |---|---|---|---|---|
/// | `Some(s)` | any | any | `Resume` | loaded state, checkpoint |
/// | `None` | `true` | `Some(_)` | `Rebuild` | initial(), None |
/// | `None` | `true` | `None` | `Fresh` | initial(), None |
/// | `None` | `false` | `Some(_)` | `Resume` | initial(), checkpoint |
/// | `None` | `false` | `None` | `Fresh` | initial(), None |
pub(crate) fn resolve_startup<S>(
    loaded_state: Option<(S, Version)>,
    last_checkpoint: Option<Version>,
    persists_state: bool,
    initial: impl FnOnce() -> S,
) -> (ProjectionStatus<S>, StartupDecision) {
    match loaded_state {
        Some((state, _)) => (
            ProjectionStatus::Idle {
                state,
                checkpoint: last_checkpoint,
            },
            StartupDecision::Resume,
        ),
        None if persists_state && last_checkpoint.is_some() => (
            ProjectionStatus::Idle {
                state: initial(),
                checkpoint: None,
            },
            StartupDecision::Rebuild,
        ),
        None if last_checkpoint.is_some() => (
            ProjectionStatus::Idle {
                state: initial(),
                checkpoint: last_checkpoint,
            },
            StartupDecision::Resume,
        ),
        None => (
            ProjectionStatus::Idle {
                state: initial(),
                checkpoint: None,
            },
            StartupDecision::Fresh,
        ),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code — relaxed lints"
)]
mod tests {
    use nexus::Version;

    use super::*;

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    #[test]
    fn resolve_resumes_when_state_loaded() {
        let (status, decision) =
            resolve_startup(Some(("loaded", v(5))), Some(v(5)), true, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "loaded");
        assert_eq!(checkpoint, Some(v(5)));
        assert_eq!(decision, StartupDecision::Resume);
    }

    #[test]
    fn resolve_resumes_when_state_loaded_regardless_of_persists_flag() {
        let (status, decision) =
            resolve_startup(Some(("loaded", v(3))), Some(v(3)), false, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "loaded");
        assert_eq!(checkpoint, Some(v(3)));
        assert_eq!(decision, StartupDecision::Resume);
    }

    #[test]
    fn resolve_rebuilds_when_state_missing_but_checkpoint_exists() {
        let (status, decision) =
            resolve_startup(None::<(&str, Version)>, Some(v(10)), true, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "initial");
        assert!(checkpoint.is_none());
        assert_eq!(decision, StartupDecision::Rebuild);
    }

    #[test]
    fn resolve_fresh_on_first_run_with_persistence() {
        let (status, decision) =
            resolve_startup(None::<(&str, Version)>, None, true, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "initial");
        assert!(checkpoint.is_none());
        assert_eq!(decision, StartupDecision::Fresh);
    }

    #[test]
    fn resolve_resumes_without_state_persistence() {
        let (status, decision) =
            resolve_startup(None::<(&str, Version)>, Some(v(7)), false, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "initial");
        assert_eq!(checkpoint, Some(v(7)));
        assert_eq!(decision, StartupDecision::Resume);
    }

    #[test]
    fn resolve_fresh_on_first_run_without_persistence() {
        let (status, decision) =
            resolve_startup(None::<(&str, Version)>, None, false, || "initial");
        let ProjectionStatus::Idle { state, checkpoint } = status else {
            panic!("expected Idle");
        };
        assert_eq!(state, "initial");
        assert!(checkpoint.is_none());
        assert_eq!(decision, StartupDecision::Fresh);
    }

    #[test]
    fn resolve_initial_is_lazy_when_state_loaded() {
        let mut called = false;
        let (status, _) = resolve_startup(Some(("loaded", v(1))), Some(v(1)), true, || {
            called = true;
            "initial"
        });
        assert!(matches!(status, ProjectionStatus::Idle { .. }));
        assert!(!called, "initial() should not be called when state is loaded");
    }
}
```

**Step 2: Register the module**

Add `mod projection;` to `crates/nexus-framework/src/projection/mod.rs` (with the other module declarations). Do NOT change re-exports yet.

**Step 3: Run tests to verify they pass**

Run: `cargo test -p nexus-framework -- projection::projection::tests`
Expected: all 7 tests pass.

**Step 4: Run full checks**

Run: `cargo fmt --all && git add crates/nexus-framework/src/projection/projection.rs crates/nexus-framework/src/projection/mod.rs && nix flake check`
Expected: all checks pass (dead_code warnings expected — new types not yet wired).

**Step 5: Commit**

```
git commit -m "feat(framework): add Projection<Mode> with Configured/Ready typestates"
```

---

### Task 2: Add `initialize()` and `run()` to `Projection`

**Files:**
- Modify: `crates/nexus-framework/src/projection/projection.rs`

**Step 1: Add `initialize()` on `Projection<..., Configured>`**

Add after the `resolve_startup` function, before `#[cfg(test)]`:

```rust
use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::status::{ProjectionStatus, apply_event};
use super::stream::{DecodeStreamError, DecodedStream};

impl<I, Sub, Ckpt, SP, P, EC, Trig> Projection<I, Sub, Ckpt, SP, P, EC, Trig, Configured>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Load checkpoint and state, resolve the startup decision.
    ///
    /// Returns `Projection<..., Ready>` with the resolved `ProjectionStatus::Idle`
    /// and the `StartupDecision` label.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Checkpoint`] or [`ProjectionError::State`]
    /// if loading fails.
    pub async fn initialize(
        self,
    ) -> Result<
        Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>,
        ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>,
    > {
        let last_checkpoint = self
            .checkpoint
            .load(&self.id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let loaded_state = self
            .state_persistence
            .load(&self.id)
            .await
            .map_err(ProjectionError::State)?;

        let (status, decision) = resolve_startup(
            loaded_state,
            last_checkpoint,
            self.state_persistence.persists_state(),
            || self.projector.initial(),
        );

        Ok(Projection {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            mode: Ready { status, decision },
        })
    }
}
```

**Step 2: Add `decision()`, `status()`, `rebuild()`, and `run()` on `Projection<..., Ready>`**

```rust
impl<I, Sub, Ckpt, SP, P, EC, Trig>
    Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>
where
    P: Projector,
{
    /// The startup decision — why the projection resolved to its current state.
    #[must_use]
    pub const fn decision(&self) -> StartupDecision {
        self.mode.decision
    }

    /// The resolved projection status (always `Idle` after initialization).
    #[must_use]
    pub const fn status(&self) -> &ProjectionStatus<P::State> {
        &self.mode.status
    }
}

impl<I, Sub, Ckpt, SP, P, EC, Trig>
    Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>
where
    P: Projector,
{
    /// Discard loaded state and reset to initial. Replays from the beginning.
    #[must_use]
    pub fn rebuild(self) -> Self {
        let initial_state = self.projector.initial();
        Projection {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            mode: Ready {
                status: ProjectionStatus::Idle {
                    state: initial_state,
                    checkpoint: None,
                },
                decision: StartupDecision::Rebuild,
            },
        }
    }
}

impl<I, Sub, Ckpt, SP, P, EC, Trig>
    Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>
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
    /// # Errors
    ///
    /// Returns [`ProjectionError`] on subscription, codec, projector,
    /// state persistence, or checkpoint failure.
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
            mode: Ready { status, .. },
        } = self;

        let stream = subscription
            .subscribe(&id, status.checkpoint())
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut status = status;

        tokio::pin!(shutdown);

        loop {
            match tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            } {
                None => {
                    if let ProjectionStatus::Pending {
                        ref state, version, ..
                    } = status
                    {
                        state_persistence
                            .save(&id, version, state)
                            .await
                            .map_err(ProjectionError::State)?;
                        checkpoint
                            .save(&id, version)
                            .await
                            .map_err(ProjectionError::Checkpoint)?;
                    }
                    return Ok(());
                }
                Some(Ok((version, event))) => {
                    status = apply_event(&projector, &trigger, status, &event, version)
                        .map_err(ProjectionError::Projector)?;

                    if let ProjectionStatus::Committed {
                        ref state, version, ..
                    } = status
                    {
                        state_persistence
                            .save(&id, version, state)
                            .await
                            .map_err(ProjectionError::State)?;
                        checkpoint
                            .save(&id, version)
                            .await
                            .map_err(ProjectionError::Checkpoint)?;
                    }
                }
                Some(Err(DecodeStreamError::Stream(e))) => {
                    return Err(ProjectionError::Subscription(e));
                }
                Some(Err(DecodeStreamError::Codec(e))) => {
                    return Err(ProjectionError::EventCodec(e));
                }
            }
        }
    }
}
```

Note: `run()` calls `status.checkpoint()` to get the resume position. We need to add a `checkpoint()` accessor to `ProjectionStatus::Idle` in `status.rs`.

**Step 3: Add `checkpoint()` accessor to `ProjectionStatus`**

In `crates/nexus-framework/src/projection/status.rs`, add after the `apply_event` function:

```rust
impl<S> ProjectionStatus<S> {
    /// The checkpoint (last persisted version), if any.
    ///
    /// Used by `Projection::run()` to determine the subscribe-from position.
    pub(crate) fn checkpoint(&self) -> Option<Version> {
        match self {
            ProjectionStatus::Idle { checkpoint, .. } => *checkpoint,
            ProjectionStatus::Pending { checkpoint, .. } => *checkpoint,
            ProjectionStatus::Committed { version, .. } => Some(*version),
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p nexus-framework -- projection::projection::tests`
Expected: all 7 tests pass. (The new methods aren't tested yet — they'll be tested via integration tests after wiring.)

**Step 5: Run full checks**

Run: `cargo fmt --all && git add crates/nexus-framework/src/projection/projection.rs crates/nexus-framework/src/projection/status.rs && nix flake check`
Expected: all checks pass.

**Step 6: Commit**

```
git commit -m "feat(framework): add initialize() and run() to Projection<Mode>"
```

---

### Task 3: Wire up builder, update mod.rs, delete old files

**Files:**
- Modify: `crates/nexus-framework/src/projection/builder.rs` (rename types)
- Modify: `crates/nexus-framework/src/projection/mod.rs` (re-exports)
- Delete: `crates/nexus-framework/src/projection/runner.rs`
- Delete: `crates/nexus-framework/src/projection/prepared.rs`
- Delete: `crates/nexus-framework/src/projection/initialized.rs`

**Step 1: Update `builder.rs`**

Replace all references to `ProjectionRunner` with `Projection` and `Configured`:

- Line 7: `use super::runner::ProjectionRunner;` → `use super::projection::{Configured, Projection};`
- Line 41: `pub struct ProjectionRunnerBuilder` → `pub struct ProjectionBuilder`
- Lines 53-63: Entry point `impl` block — change `ProjectionRunner<I, NeedsSub, ...>` to `Projection<I, NeedsSub, ..., Configured>`
- Line 66: `pub const fn builder(id: I) -> ProjectionRunnerBuilder<...>` stays on `Projection`
- Line 77: return `ProjectionRunnerBuilder { ... }` → `ProjectionBuilder { ... }`
- Lines 91, 163, 227: all `impl ProjectionRunnerBuilder<...>` → `impl ProjectionBuilder<...>`
- Lines 98, 115, 132, 149, 174, 197: all struct construction `ProjectionRunnerBuilder { ... }` → `ProjectionBuilder { ... }`
- Line 240: `.build()` returns `ProjectionRunner<...>` → `Projection<..., Configured>`
- Line 241: `ProjectionRunner { ... }` → `Projection { ..., mode: Configured }`

**Step 2: Update `mod.rs`**

Replace the entire file:

```rust
mod builder;
mod error;
mod persist;
mod projection;
mod status;
mod stream;

pub use builder::ProjectionBuilder;
pub use error::{ProjectionError, StatePersistError};
pub use persist::{NoStatePersistence, StatePersistence, WithStatePersistence};
pub use projection::{Configured, Projection, Ready, StartupDecision};
```

**Step 3: Delete old files**

```bash
git rm crates/nexus-framework/src/projection/runner.rs
git rm crates/nexus-framework/src/projection/prepared.rs
git rm crates/nexus-framework/src/projection/initialized.rs
```

**Step 4: Run unit tests**

Run: `cargo test -p nexus-framework --lib`
Expected: all unit tests pass (status::tests + projection::tests). Integration tests will fail — that's Task 4.

**Step 5: Run `cargo fmt --all`**

**Step 6: Commit (without full nix check — integration tests need updating)**

```
git add -A && git commit -m "refactor(framework): wire Projection<Mode>, delete runner/prepared/initialized"
```

Note: `nix flake check` will fail because integration tests still reference old types. That's expected — Task 4 fixes them.

---

### Task 4: Update integration tests

**Files:**
- Modify: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Update imports (line 17-19)**

```rust
// Before
use nexus_framework::projection::{
    Initialized, NoStatePersistence, ProjectionError, ProjectionRunner, StatePersistence,
    WithStatePersistence,
};

// After
use nexus_framework::projection::{
    NoStatePersistence, Projection, ProjectionError, StartupDecision, StatePersistence,
    WithStatePersistence,
};
```

**Step 2: Find-and-replace `ProjectionRunner::builder` → `Projection::builder`**

All instances in the file. Mechanical rename — every `ProjectionRunner::builder(` becomes `Projection::builder(`.

**Step 3: Update typestate/startup decision tests (lines 1254-1497)**

The 4 tests that match on `Initialized` variants need to use `decision()` instead:

`initialize_returns_starting_on_first_run` (line 1255):
```rust
let ready = projection.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Fresh);
```

`initialize_returns_resuming_after_successful_run` (line 1271):
```rust
let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Resume);
```

`initialize_returns_rebuilding_on_schema_mismatch` (line 1309):
```rust
let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Rebuild);
```

`initialize_returns_resuming_without_state_persistence` (line 1348):
```rust
let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Resume);
```

`force_rebuild_replays_from_beginning` (line 1383):
```rust
let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Resume);
// Rebuild and run
ready
    .rebuild()
    .run(async { ... })
    .await
    .unwrap();
```

`resuming_accessors_return_correct_values` (line 1452):
```rust
use nexus_framework::projection::projection::ProjectionStatus; // if needed, or access via status()
let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Resume);
// Access state via status()
let status = ready.status();
// ProjectionStatus::Idle { state, checkpoint } — need to check how to access
```

Note: The `resuming_accessors_return_correct_values` test accessed `prepared.resume_from()` and `prepared.state()`. With the new API, these are accessed through `ready.status()` which returns `&ProjectionStatus<CountState>`. We need to make `ProjectionStatus` public (currently `pub(crate)`) for this test to work, OR add accessor methods on `Ready`.

**Decision: Make `ProjectionStatus` `pub` and export it.** The supervisor needs to inspect the status — that's the whole point of the two-phase API.

Update `status.rs` line 14: `pub(crate) enum ProjectionStatus<S>` → `pub enum ProjectionStatus<S>`
Update `mod.rs`: add `pub use status::ProjectionStatus;`

The test becomes:
```rust
use nexus_framework::projection::ProjectionStatus;

let ready = runner2.initialize().await.unwrap();
assert_eq!(ready.decision(), StartupDecision::Resume);
let ProjectionStatus::Idle { ref state, checkpoint } = *ready.status() else {
    panic!("expected Idle");
};
assert_eq!(checkpoint, Some(Version::new(1).unwrap()));
assert_eq!(state, &CountState { count: 1, total: 10 });
```

**Step 4: Rename test file**

```bash
git mv crates/nexus-framework/tests/projection_runner_tests.rs crates/nexus-framework/tests/projection_tests.rs
```

**Step 5: Run tests**

Run: `cargo test -p nexus-framework`
Expected: all tests pass.

**Step 6: Run full checks**

Run: `cargo fmt --all && git add -A && nix flake check`
Expected: all checks pass.

**Step 7: Commit**

```
git commit -m "refactor(framework): update integration tests for Projection<Mode> API"
```

---

### Task 5: Update CLAUDE.md architecture section

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update the Framework Crate projection subsection**

Replace the projection bullet points in the `### Framework Crate` section with:

```
- **`projection/`** — Subscription-powered CQRS projections (feature-gated: `projection`).
  - `projection.rs` — `Projection<I, Sub, Ckpt, SP, P, EC, Trig, Mode>`: two-phase typestate. `Configured` (built, not loaded) → `initialize()` → `Ready<S>` (loaded, can run). `Ready` carries `ProjectionStatus<S>` and `StartupDecision` (Fresh/Resume/Rebuild). `run()` subscribes and enters the event loop. `rebuild()` resets to initial state.
  - `status.rs` — `ProjectionStatus<S>` enum: explicit FSM for the event loop with three write-centric states (`Idle`, `Pending`, `Committed`). Pure sync `apply_event` transition function — no IO, no async. Driven by the async shell in `Projection::run()`.
  - `builder.rs` — `ProjectionBuilder`: typestate builder with `!Send` markers for compile-time required field enforcement.
  - `error.rs` — `ProjectionError<P, EC, SP, Ckpt, Sub>`: one variant per failure domain. `StatePersistError<S, C>`.
  - `persist.rs` — `StatePersistence<S>` trait: `NoStatePersistence` (Infallible) and `WithStatePersistence<SS, SC>`.
  - `stream.rs` — `DecodedStream`: adapter converting lending GAT `EventStream` to owned `tokio_stream::Stream` by decoding inside `poll_next`.
```

**Step 2: Run full checks**

Run: `nix flake check`
Expected: all checks pass.

**Step 3: Commit**

```
git commit -m "docs: update CLAUDE.md for Projection<Mode> API"
```
