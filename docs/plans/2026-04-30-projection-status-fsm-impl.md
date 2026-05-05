# ProjectionStatus FSM Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace scattered `mut` variables and opaque `(State, bool)` tuple in the projection event loop with an explicit `ProjectionStatus<S>` FSM enum (Idle, Pending, Committed).

**Architecture:** Pure sync transition function `apply_event` in `status.rs` produces `ProjectionStatus` values. Async shell in `prepared.rs` interprets them: `Committed` → save, `Pending` on shutdown → flush. No bools, one `mut` variable.

**Tech Stack:** Rust, nexus-framework crate, nexus/nexus-store traits

**Design doc:** `docs/plans/2026-04-30-projection-status-fsm-design.md`

---

### Task 1: Create `status.rs` with `ProjectionStatus` enum and `apply_event`

**Files:**
- Create: `crates/nexus-framework/src/projection/status.rs`
- Modify: `crates/nexus-framework/src/projection/mod.rs:1` (add `mod status;`)

**Step 1: Write the failing tests**

Create `status.rs` with the test module and fixtures. Write tests for all 6 event transitions from the design doc's state transition table, plus error propagation:

```rust
use std::iter;

use nexus::{DomainEvent, Version};
use nexus_store::projection::{ProjectionTrigger, Projector};

/// FSM state for the projection event loop.
///
/// Three write-centric states tracking the relationship between
/// in-memory folded state and persisted state:
///
/// - [`Idle`](ProjectionStatus::Idle) — no events processed yet
/// - [`Pending`](ProjectionStatus::Pending) — events folded, write pending
/// - [`Committed`](ProjectionStatus::Committed) — trigger fired, state persisted
pub(crate) enum ProjectionStatus<S> {
    /// No events processed yet.
    Idle {
        state: S,
        checkpoint: Option<Version>,
    },
    /// Events folded, write pending.
    Pending {
        state: S,
        version: Version,
        checkpoint: Option<Version>,
    },
    /// Trigger fired, state persisted at version.
    Committed {
        state: S,
        version: Version,
    },
}

/// Stub — tests should fail until Step 3.
pub(crate) fn apply_event<P, E, Trig>(
    _projector: &P,
    _trigger: &Trig,
    _status: ProjectionStatus<P::State>,
    _event: &E,
    _version: Version,
) -> Result<ProjectionStatus<P::State>, P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    todo!()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code — relaxed lints"
)]
mod tests {
    use std::convert::Infallible;
    use std::num::NonZeroU64;

    use nexus::{DomainEvent, Message, Version};
    use nexus_store::projection::{EveryNEvents, ProjectionTrigger, Projector};

    use super::*;

    // ── Fixtures (same as prepared.rs, shared domain) ─────────────────

    #[derive(Debug)]
    struct Evt;
    impl Message for Evt {}
    impl DomainEvent for Evt {
        fn name(&self) -> &'static str {
            "Evt"
        }
    }

    struct SumProjector;
    impl Projector for SumProjector {
        type Event = Evt;
        type State = u64;
        type Error = Infallible;
        fn initial(&self) -> u64 { 0 }
        fn apply(&self, state: u64, _event: &Evt) -> Result<u64, Infallible> {
            Ok(state.wrapping_add(1))
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("apply failed")]
    struct ApplyError;

    struct FailingProjector;
    impl Projector for FailingProjector {
        type Event = Evt;
        type State = u64;
        type Error = ApplyError;
        fn initial(&self) -> u64 { 0 }
        fn apply(&self, _state: u64, _event: &Evt) -> Result<u64, ApplyError> {
            Err(ApplyError)
        }
    }

    struct AlwaysTrigger;
    impl ProjectionTrigger for AlwaysTrigger {
        fn should_project(
            &self, _old: Option<Version>, _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool { true }
    }

    struct NeverTrigger;
    impl ProjectionTrigger for NeverTrigger {
        fn should_project(
            &self, _old: Option<Version>, _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool { false }
    }

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    // ── 1. Sequence/Protocol: all 6 event transitions ─────────────────

    #[test]
    fn idle_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Idle { state: 0_u64, checkpoint: None };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(1)).unwrap();
        let ProjectionStatus::Pending { state, version, checkpoint } = result else {
            panic!("expected Pending");
        };
        assert_eq!(state, 1);
        assert_eq!(version, v(1));
        assert!(checkpoint.is_none());
    }

    #[test]
    fn idle_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Idle { state: 0_u64, checkpoint: None };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(1)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 1);
        assert_eq!(version, v(1));
    }

    #[test]
    fn pending_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Pending {
            state: 1_u64, version: v(1), checkpoint: None,
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(2)).unwrap();
        let ProjectionStatus::Pending { state, version, checkpoint } = result else {
            panic!("expected Pending");
        };
        assert_eq!(state, 2);
        assert_eq!(version, v(2));
        assert!(checkpoint.is_none());
    }

    #[test]
    fn pending_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Pending {
            state: 1_u64, version: v(1), checkpoint: None,
        };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(2)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 2);
        assert_eq!(version, v(2));
    }

    #[test]
    fn committed_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Committed { state: 3_u64, version: v(3) };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Pending { state, version, checkpoint } = result else {
            panic!("expected Pending");
        };
        assert_eq!(state, 4);
        assert_eq!(version, v(4));
        assert_eq!(checkpoint, Some(v(3)));
    }

    #[test]
    fn committed_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Committed { state: 3_u64, version: v(3) };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 4);
        assert_eq!(version, v(4));
    }

    // ── Multi-step sequence ───────────────────────────────────────────

    #[test]
    fn multi_step_idle_pending_pending_committed_pending_committed() {
        let every_3 = EveryNEvents(NonZeroU64::new(3).unwrap());

        // Idle → Pending (v1, no trigger at bucket 0)
        let s = ProjectionStatus::Idle { state: 0_u64, checkpoint: None };
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(1)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Pending (v2, still bucket 0)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(2)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Committed (v3, crosses to bucket 1)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(3)).unwrap();
        assert!(matches!(s, ProjectionStatus::Committed { .. }));

        // Committed → Pending (v4, bucket 1 still)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(4)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Committed (v6, crosses to bucket 2)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(6)).unwrap();
        let ProjectionStatus::Committed { state, version } = s else {
            panic!("expected Committed");
        };
        assert_eq!(state, 5);
        assert_eq!(version, v(6));
    }

    // ── 3. Defensive: error propagation from each variant ─────────────

    #[test]
    fn error_propagated_from_idle() {
        let status = ProjectionStatus::Idle { state: 0_u64, checkpoint: None };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(1)).is_err());
    }

    #[test]
    fn error_propagated_from_pending() {
        let status = ProjectionStatus::Pending {
            state: 0_u64, version: v(1), checkpoint: None,
        };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(2)).is_err());
    }

    #[test]
    fn error_propagated_from_committed() {
        let status = ProjectionStatus::Committed { state: 0_u64, version: v(1) };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(2)).is_err());
    }

    // ── Checkpoint correctness ────────────────────────────────────────

    #[test]
    fn checkpoint_preserved_through_pending_transitions() {
        let status = ProjectionStatus::Idle { state: 0_u64, checkpoint: Some(v(5)) };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(6)).unwrap();
        let ProjectionStatus::Pending { checkpoint, .. } = result else {
            panic!("expected Pending");
        };
        assert_eq!(checkpoint, Some(v(5)));
    }

    #[test]
    fn committed_version_becomes_checkpoint_on_next_pending() {
        // Commit at v(3), then no-trigger at v(4) → Pending with checkpoint = v(3)
        let status = ProjectionStatus::Committed { state: 3_u64, version: v(3) };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Pending { checkpoint, .. } = result else {
            panic!("expected Pending");
        };
        assert_eq!(checkpoint, Some(v(3)));
    }
}
```

**Step 2: Register the module**

Add `mod status;` to `crates/nexus-framework/src/projection/mod.rs` (line 1, with the other module declarations).

**Step 3: Run tests to verify they fail**

Run: `cargo test -p nexus-framework -- status::tests`
Expected: all tests panic with `not yet implemented` from the `todo!()` stub.

**Step 4: Implement `apply_event`**

Replace the `todo!()` stub with the real implementation:

```rust
pub(crate) fn apply_event<P, E, Trig>(
    projector: &P,
    trigger: &Trig,
    status: ProjectionStatus<P::State>,
    event: &E,
    version: Version,
) -> Result<ProjectionStatus<P::State>, P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    let (state, checkpoint) = match status {
        ProjectionStatus::Idle { state, checkpoint } => (state, checkpoint),
        ProjectionStatus::Pending { state, checkpoint, .. } => (state, checkpoint),
        ProjectionStatus::Committed { state, version } => (state, Some(version)),
    };

    let new_state = projector.apply(state, event)?;
    let should_persist =
        trigger.should_project(checkpoint, version, iter::once(event.name()));

    if should_persist {
        Ok(ProjectionStatus::Committed { state: new_state, version })
    } else {
        Ok(ProjectionStatus::Pending {
            state: new_state,
            version,
            checkpoint,
        })
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p nexus-framework -- status::tests`
Expected: all 12 tests pass.

**Step 6: Run full checks**

Run: `nix flake check`
Expected: all checks pass (clippy, fmt, tests, etc.)

**Step 7: Commit**

```
git add crates/nexus-framework/src/projection/status.rs crates/nexus-framework/src/projection/mod.rs
git commit -m "feat(framework): add ProjectionStatus FSM enum with apply_event"
```

---

### Task 2: Refactor `PreparedProjection::run()` to use `ProjectionStatus`

**Files:**
- Modify: `crates/nexus-framework/src/projection/prepared.rs:1-57` (remove old `apply_event`)
- Modify: `crates/nexus-framework/src/projection/prepared.rs:110-200` (refactor `run()` loop)

**Step 1: Remove old `apply_event` from `prepared.rs`**

Delete lines 27-57 (the `// Sync core` section with the old `apply_event` function).

Remove `use std::iter;` from line 1 (no longer needed — `iter` usage moved to `status.rs`).

**Step 2: Add import for `status` module**

Add to the imports at the top of `prepared.rs`:

```rust
use super::status::{ProjectionStatus, apply_event};
```

**Step 3: Refactor the `run()` method loop body**

Replace the loop body in `run()` (lines 133-199) with the FSM-driven shell:

```rust
        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut status = ProjectionStatus::Idle {
            state,
            checkpoint: resume_from,
        };

        tokio::pin!(shutdown);

        loop {
            match tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            } {
                None => {
                    if let ProjectionStatus::Pending { ref state, version, .. } = status {
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
                    status = apply_event(
                        &projector, &trigger, status, &event, version,
                    )
                    .map_err(ProjectionError::Projector)?;
                    if let ProjectionStatus::Committed { ref state, version } = status {
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
```

**Step 4: Run tests**

Run: `cargo test -p nexus-framework`
Expected: all unit tests AND integration tests pass. The async shell behavior is identical — only the internal state tracking changed.

**Step 5: Run full checks**

Run: `nix flake check`
Expected: all checks pass.

**Step 6: Commit**

```
git add crates/nexus-framework/src/projection/prepared.rs
git commit -m "refactor(framework): drive event loop with ProjectionStatus FSM"
```

---

### Task 3: Migrate unit tests from `prepared.rs` to `status.rs`

**Files:**
- Modify: `crates/nexus-framework/src/projection/prepared.rs:265-442` (test module)

**Step 1: Remove old `apply_event` tests from `prepared.rs`**

Delete the `apply_event` tests section (lines 354-394 approximately):
- `apply_event_folds_state_through_projector`
- `apply_event_returns_should_persist_true_when_trigger_fires`
- `apply_event_returns_should_persist_false_when_trigger_does_not_fire`
- `apply_event_propagates_projector_error`
- `apply_event_passes_versions_to_trigger`

Also remove the now-unused test fixtures from `prepared.rs::tests` that are duplicated in `status.rs::tests`:
- `Evt`, `SumProjector`, `ApplyError`, `FailingProjector`, `AlwaysTrigger`, `NeverTrigger`, `v()`

Keep only `make_resuming_prepared()` and the 4 typestate tests that use it (these still test `PreparedProjection`-specific behavior). Those tests need their own minimal fixtures — add back only what they need.

**Step 2: Update `prepared.rs` test imports**

The remaining typestate tests (`resuming_state_returns_loaded_state`, `resuming_resume_from_returns_checkpoint`, `force_rebuild_*`) only need `PreparedProjection`, `SumProjector`, `NeverTrigger`, `Resuming`, `PhantomData`, and `v()`. Since they test `PreparedProjection` struct construction and typestate methods, they need their own `SumProjector` fixture (it's used in `make_resuming_prepared`). Keep a minimal copy of the required fixtures.

**Step 3: Run tests**

Run: `cargo test -p nexus-framework`
Expected: all tests pass — old `apply_event` tests now live in `status.rs`, typestate tests remain in `prepared.rs`.

**Step 4: Run full checks**

Run: `nix flake check`
Expected: all checks pass.

**Step 5: Commit**

```
git add crates/nexus-framework/src/projection/prepared.rs
git commit -m "refactor(framework): migrate apply_event tests to status module"
```

---

### Task 4: Update CLAUDE.md architecture section

**Files:**
- Modify: `CLAUDE.md` (Framework Crate section for projection)

**Step 1: Update the projection module description**

In the `### Framework Crate` section, update the `projection/` subsection to document `status.rs`:

Add after the `prepared.rs` bullet:

```
  - `status.rs` — `ProjectionStatus<S>` enum: explicit FSM for the event loop with three write-centric states (`Idle`, `Pending`, `Committed`). Pure sync `apply_event` transition function — no IO, no async. Driven by the async shell in `prepared.rs`.
```

**Step 2: Run full checks**

Run: `nix flake check`
Expected: all checks pass.

**Step 3: Commit**

```
git add CLAUDE.md
git commit -m "docs: document ProjectionStatus FSM in architecture section"
```
