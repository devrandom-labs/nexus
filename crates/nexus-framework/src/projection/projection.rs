use nexus::Id;
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

impl<S> Ready<S> {
    pub(crate) fn new(status: ProjectionStatus<S>, decision: StartupDecision) -> Self {
        Self { status, decision }
    }
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

impl<I, Sub, Ckpt, SP, P, EC, Trig, Mode> Projection<I, Sub, Ckpt, SP, P, EC, Trig, Mode> {
    pub(crate) fn new(
        id: I,
        subscription: Sub,
        checkpoint: Ckpt,
        state_persistence: SP,
        projector: P,
        event_codec: EC,
        trigger: Trig,
        mode: Mode,
    ) -> Self {
        Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
            mode,
        }
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
    /// # Decision table
    ///
    /// | loaded_state | persists_state | checkpoint | decision | Idle state |
    /// |---|---|---|---|---|
    /// | `Some(s)` | any | any | `Resume` | loaded state, checkpoint |
    /// | `None` | `true` | `Some(_)` | `Rebuild` | initial(), None |
    /// | `None` | `true` | `None` | `Fresh` | initial(), None |
    /// | `None` | `false` | `Some(_)` | `Resume` | initial(), checkpoint |
    /// | `None` | `false` | `None` | `Fresh` | initial(), None |
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

        let persists_state = self.state_persistence.persists_state();

        let (state, checkpoint, decision) = match loaded_state {
            Some((state, _)) => (state, last_checkpoint, StartupDecision::Resume),
            None if persists_state && last_checkpoint.is_some() => {
                (self.projector.initial(), None, StartupDecision::Rebuild)
            }
            None if last_checkpoint.is_some() => (
                self.projector.initial(),
                last_checkpoint,
                StartupDecision::Resume,
            ),
            None => (self.projector.initial(), None, StartupDecision::Fresh),
        };

        Ok(Projection::new(
            self.id,
            self.subscription,
            self.checkpoint,
            self.state_persistence,
            self.projector,
            self.event_codec,
            self.trigger,
            Ready::new(ProjectionStatus::Idle { state, checkpoint }, decision),
        ))
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
        Projection::new(
            self.id,
            self.subscription,
            self.checkpoint,
            self.state_persistence,
            self.projector,
            self.event_codec,
            self.trigger,
            Ready::new(
                ProjectionStatus::Idle {
                    state: initial_state,
                    checkpoint: None,
                },
                StartupDecision::Rebuild,
            ),
        )
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
