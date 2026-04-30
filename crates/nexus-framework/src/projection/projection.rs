use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::status::{ProjectionStatus, apply_event};
use super::stream::{DecodeStreamError, DecodedStream};

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
pub struct Projection<I, Sub, Ckpt, SP, P, EC, Trig, Mode> {
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

// ═══════════════════════════════════════════════════════════════════════════
// Projection<Configured> — load checkpoint + state, resolve startup
// ═══════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════
// Projection<Ready> — inspect, rebuild, run
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, Ckpt, SP, P, EC, Trig> Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>
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

impl<I, Sub, Ckpt, SP, P, EC, Trig> Projection<I, Sub, Ckpt, SP, P, EC, Trig, Ready<P::State>>
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
