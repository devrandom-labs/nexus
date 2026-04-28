use std::iter;

use nexus::{DomainEvent, Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::stream::{DecodeStreamError, DecodedStream};

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
    #[expect(
        clippy::too_many_lines,
        reason = "run() is being replaced by PreparedProjection::run() in upcoming tasks"
    )]
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
        } = self;

        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let loaded_state = state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?;

        let (mut state, resume_from) = match resolve_startup(
            loaded_state,
            last_checkpoint,
            state_persistence.persists_state(),
            || projector.initial(),
        ) {
            StartupOutcome::Resuming { state, resume_from } => (state, resume_from),
            StartupOutcome::Rebuilding { state } | StartupOutcome::Starting { state } => {
                (state, None)
            }
        };

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

            let event_name = event.name();
            state = projector
                .apply(state, &event)
                .map_err(ProjectionError::Projector)?;
            current_version = Some(version);
            dirty = true;

            if trigger.should_project(last_persisted_version, version, iter::once(event_name)) {
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
