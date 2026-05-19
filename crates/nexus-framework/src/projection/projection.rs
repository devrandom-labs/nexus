use std::num::NonZeroU32;

use nexus::Id;
use nexus_store::Codec;
use nexus_store::projection::Projector;
use nexus_store::state::{PersistTrigger, State, StateStore};
use nexus_store::store::{CheckpointStore, Subscription};
use nexus_store::stream::EventStreamExt;

use super::error::ProjectionError;
use super::status::{ProjectionStatus, apply_event};

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
    pub(crate) state_store: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) schema_version: NonZeroU32,
    pub(crate) persists_state: bool,
    pub(crate) mode: Mode,
}

impl<I, Sub, Ckpt, SP, P, EC, Trig, Mode> Projection<I, Sub, Ckpt, SP, P, EC, Trig, Mode> {
    pub(crate) fn new(
        id: I,
        subscription: Sub,
        checkpoint: Ckpt,
        state_store: SP,
        projector: P,
        event_codec: EC,
        trigger: Trig,
        schema_version: NonZeroU32,
        persists_state: bool,
        mode: Mode,
    ) -> Self {
        Self {
            id,
            subscription,
            checkpoint,
            state_store,
            projector,
            event_codec,
            trigger,
            schema_version,
            persists_state,
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
    SP: StateStore<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: PersistTrigger,
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
            .state_store
            .load(&self.id, self.schema_version)
            .await
            .map_err(ProjectionError::State)?;

        let (state, checkpoint, decision) = match loaded_state {
            Some(persisted) => {
                let (_version, _schema, state) = persisted.into_parts();
                (state, last_checkpoint, StartupDecision::Resume)
            }
            None if self.persists_state && last_checkpoint.is_some() => {
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
            self.state_store,
            self.projector,
            self.event_codec,
            self.trigger,
            self.schema_version,
            self.persists_state,
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
            self.state_store,
            self.projector,
            self.event_codec,
            self.trigger,
            self.schema_version,
            self.persists_state,
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
    I: Id + Clone + Send + Sync,
    Sub: Subscription<()> + Send,
    Ckpt: CheckpointStore + Send + Sync,
    SP: StateStore<P::State> + Send + Sync,
    P: Projector + Send + Sync,
    P::State: Clone + Send,
    EC: Codec<P::Event> + Send + Sync,
    Trig: PersistTrigger + Send + Sync,
{
    /// Run the event loop until shutdown or error.
    ///
    /// Drives the subscription through a single
    /// [`try_fold_async_until`](EventStreamExt::try_fold_async_until)
    /// invocation. The fold body is the *only* place projection-specific
    /// logic lives:
    ///
    /// 1. Decode the envelope.
    /// 2. Run the pure [`apply_event`] FSM transition.
    /// 3. If the new status is `Committed`, persist state + checkpoint.
    ///
    /// When the shutdown future resolves, the combinator returns the
    /// accumulator at the last completed iteration with
    /// [`Disposition::Interrupted`](nexus_store::stream::Disposition::Interrupted).
    /// If that accumulator is `Pending`, the flush tail persists it once —
    /// honoring the same fs2-style "preserve partial accumulator and let
    /// the caller finalize" contract.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] on subscription, codec, projector,
    /// state persistence, or checkpoint failure.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            checkpoint,
            state_store,
            projector,
            event_codec,
            trigger,
            schema_version,
            mode: Ready { status, .. },
            ..
        } = self;

        let mut stream = subscription
            .subscribe(&id, status.checkpoint())
            .await
            .map_err(ProjectionError::Subscription)?;

        // Borrow the ingredients so the closure captures references
        // (each iteration call has access; ownership stays out here for
        // the flush tail).
        let projector_ref = &projector;
        let trigger_ref = &trigger;
        let event_codec_ref = &event_codec;
        let state_store_ref = &state_store;
        let checkpoint_ref = &checkpoint;
        let id_ref = &id;

        let (final_status, _disposition) = stream
            .try_fold_async_until(
                status,
                move |status, envelope| {
                    // Sync prelude: decode into owned values, drop the
                    // envelope before the async move. The returned future
                    // must not borrow from the envelope (HRTB requirement
                    // for try_fold_async_until's `F: for<'a> FnMut(...) -> Fut`
                    // with a single concrete Fut).
                    let version = envelope.version();
                    let decoded = event_codec_ref.decode(envelope.event_type(), envelope.payload());
                    async move {
                        let event = decoded.map_err(ProjectionError::EventCodec)?;
                        let next = apply_event(projector_ref, trigger_ref, status, &event, version)
                            .map_err(ProjectionError::Projector)?;
                        if let ProjectionStatus::Committed {
                            ref state, version, ..
                        } = next
                        {
                            let pending = State::new(version, schema_version, state.clone());
                            state_store_ref
                                .save(id_ref, &pending)
                                .await
                                .map_err(ProjectionError::State)?;
                            checkpoint_ref
                                .save(id_ref, version)
                                .await
                                .map_err(ProjectionError::Checkpoint)?;
                        }
                        Ok::<
                            ProjectionStatus<P::State>,
                            ProjectionError<
                                P::Error,
                                EC::Error,
                                SP::Error,
                                Ckpt::Error,
                                Sub::Error,
                            >,
                        >(next)
                    }
                },
                shutdown,
            )
            .await?;

        // Flush tail: if the fold exited with pending work, persist it once
        // before returning. Mirrors fs2's finalizer guarantee — resource
        // cleanup runs regardless of completion or interrupt.
        if let ProjectionStatus::Pending {
            ref state, version, ..
        } = final_status
        {
            let pending = State::new(version, schema_version, state.clone());
            state_store
                .save(&id, &pending)
                .await
                .map_err(ProjectionError::State)?;
            checkpoint
                .save(&id, version)
                .await
                .map_err(ProjectionError::Checkpoint)?;
        }
        Ok(())
    }
}
