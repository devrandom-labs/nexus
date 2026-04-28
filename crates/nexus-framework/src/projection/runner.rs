use std::marker::PhantomData;

use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};

use super::error::ProjectionError;
use super::initialized::Initialized;
use super::persist::StatePersistence;
use super::prepared::PreparedProjection;

/// Subscription-powered async projection runner.
///
/// Subscribes to an event stream, decodes events, folds them through a
/// [`Projector`], persists state via [`StatePersistence`], and checkpoints
/// progress. Runs until the shutdown signal fires or an error occurs.
///
/// Constructed via [`ProjectionRunner::builder`]. The runner is single-use:
/// `run` consumes `self`. Restart by building a new runner.
pub struct ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
}

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
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Checkpoint`] or [`ProjectionError::State`]
    /// if loading fails.
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
            StartupOutcome::Starting { state } => Ok(Initialized::Starting(PreparedProjection {
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
            })),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// StartupOutcome — typed result of resolve_startup
// ═══════════════════════════════════════════════════════════════════════════

/// Startup outcome from resolving checkpoint + persisted state.
///
/// Private to this module. Maps directly to the public [`Initialized`] variants.
enum StartupOutcome<S> {
    /// Resume from checkpoint with loaded or initial state.
    Resuming {
        state: S,
        resume_from: Option<Version>,
    },
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
        None if persists_state && last_checkpoint.is_some() => {
            StartupOutcome::Rebuilding { state: initial() }
        }
        None if last_checkpoint.is_some() => StartupOutcome::Resuming {
            state: initial(),
            resume_from: last_checkpoint,
        },
        None => StartupOutcome::Starting { state: initial() },
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

    use super::{StartupOutcome, resolve_startup};

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

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
        let outcome = resolve_startup(None::<(&str, Version)>, Some(v(10)), true, || "initial");
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
        let outcome = resolve_startup(None::<(&str, Version)>, Some(v(7)), false, || "initial");
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
}
