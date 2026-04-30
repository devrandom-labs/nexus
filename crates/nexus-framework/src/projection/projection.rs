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
        let (status, decision) = resolve_startup(None::<(&str, Version)>, None, true, || "initial");
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
        assert!(
            !called,
            "initial() should not be called when state is loaded"
        );
    }
}
