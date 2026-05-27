mod builder;
mod error;
mod status;

use std::num::NonZeroU32;

use nexus::{Id, Version};
use nexus_store::Decode;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::projection::Projector;
use nexus_store::state::{PersistTrigger, SnapshotStore};
use nexus_store::store::Subscription;
use nexus_store::stream::{EventStream, EventStreamExt};

pub use builder::ProjectionBuilder;
pub use error::ProjectionError;
pub use status::ProjectionStatus;

use status::apply_event;

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers
// ═══════════════════════════════════════════════════════════════════════════

/// Configured but not yet loaded. Produced by [`ProjectionBuilder::build`](super::ProjectionBuilder::build).
pub struct Configured;

/// The startup decision label — why the projection resolved to its current state.
///
/// The label is for supervisor inspection only — it has no behavioral
/// effect on `run()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupDecision {
    /// First run — no snapshot persisted.
    Fresh,
    /// Loaded a snapshot. Resuming from where we left off.
    Resume,
    /// Explicit rebuild requested — replaying from the beginning.
    Rebuild,
}

/// Loaded and ready to run. Produced by [`Projection::initialize`].
pub struct Ready<S> {
    pub(crate) status: ProjectionStatus<S>,
    pub(crate) decision: StartupDecision,
}

impl<S> Ready<S> {
    pub(crate) const fn new(status: ProjectionStatus<S>, decision: StartupDecision) -> Self {
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
pub struct Projection<I, Sub, SS, P, EC, Trig, Mode> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) snapshot_store: SS,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) schema_version: NonZeroU32,
    pub(crate) mode: Mode,
}

impl<I, Sub, SS, P, EC, Trig, Mode> Projection<I, Sub, SS, P, EC, Trig, Mode> {
    #[allow(
        clippy::too_many_arguments,
        reason = "internal constructor — every field is a distinct generic component required at construction"
    )]
    pub(crate) const fn new(
        id: I,
        subscription: Sub,
        snapshot_store: SS,
        projector: P,
        event_codec: EC,
        trigger: Trig,
        schema_version: NonZeroU32,
        mode: Mode,
    ) -> Self {
        Self {
            id,
            subscription,
            snapshot_store,
            projector,
            event_codec,
            trigger,
            schema_version,
            mode,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Projection<Configured> — hydrate the snapshot, resolve startup
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, SS, P, EC, Trig> Projection<I, Sub, SS, P, EC, Trig, Configured>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    SS: SnapshotStore<P::State, Version>,
    P: Projector,
    EC: Decode<P::Event>,
    Trig: PersistTrigger,
{
    /// Hydrate the snapshot and resolve the startup decision.
    ///
    /// Returns `Projection<..., Ready>` with the resolved
    /// `ProjectionStatus::Idle` and the `StartupDecision` label.
    ///
    /// # Decision table
    ///
    /// | `hydrate`      | decision | Idle state                   |
    /// |----------------|----------|------------------------------|
    /// | `Some((v, s))` | `Resume` | loaded state, checkpoint `v` |
    /// | `None`         | `Fresh`  | `initial()`, no checkpoint   |
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Snapshot`] if hydration fails.
    pub async fn initialize(
        self,
    ) -> Result<
        Projection<I, Sub, SS, P, EC, Trig, Ready<P::State>>,
        ProjectionError<P::Error, EC::Error, SS::Error, Sub::Error>,
    > {
        let loaded = self
            .snapshot_store
            .hydrate(&self.id, self.schema_version)
            .await
            .map_err(ProjectionError::Snapshot)?;

        let (state, checkpoint, decision) = match loaded {
            Some((version, state)) => (state, Some(version), StartupDecision::Resume),
            None => (self.projector.initial(), None, StartupDecision::Fresh),
        };

        Ok(Projection::new(
            self.id,
            self.subscription,
            self.snapshot_store,
            self.projector,
            self.event_codec,
            self.trigger,
            self.schema_version,
            Ready::new(ProjectionStatus::Idle { state, checkpoint }, decision),
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Projection<Ready> — inspect, rebuild, run
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, SS, P, EC, Trig> Projection<I, Sub, SS, P, EC, Trig, Ready<P::State>>
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
        Self::new(
            self.id,
            self.subscription,
            self.snapshot_store,
            self.projector,
            self.event_codec,
            self.trigger,
            self.schema_version,
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

impl<I, Sub, SS, P, EC, Trig> Projection<I, Sub, SS, P, EC, Trig, Ready<P::State>>
where
    I: Id + Clone + Send + Sync,
    Sub: Subscription<()> + Send,
    Sub::Stream: for<'a> EventStream<(), Item<'a> = PersistedEnvelope<'a, ()>>,
    SS: SnapshotStore<P::State, Version> + Send + Sync,
    P: Projector + Send + Sync,
    P::State: Clone + Send,
    EC: Decode<P::Event> + Send + Sync,
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
    /// 3. If the new status is `Committed`, persist the snapshot —
    ///    state and position together — in one atomic `commit`.
    ///
    /// When the shutdown future resolves, the combinator returns the
    /// accumulator at the last completed iteration. If that accumulator
    /// is `Pending`, the flush tail commits it once.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] on subscription, codec, projector,
    /// or snapshot store failure.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SS::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            snapshot_store,
            projector,
            event_codec,
            trigger,
            schema_version,
            mode: Ready { status, .. },
        } = self;

        let mut stream = subscription
            .subscribe(&id, status.checkpoint())
            .await
            .map_err(ProjectionError::Subscription)?;

        // Borrow the ingredients so the closure captures references
        // (ownership stays out here for the flush tail).
        let projector_ref = &projector;
        let trigger_ref = &trigger;
        let event_codec_ref = &event_codec;
        let snapshot_store_ref = &snapshot_store;
        let id_ref = &id;

        let (final_status, _disposition) = stream
            .try_fold_async_until(
                status,
                move |acc, item| {
                    // Sync prelude: decode into owned values before the
                    // async move. The returned future must not borrow from
                    // `item` (HRTB requirement for `try_fold_async_until`'s
                    // single concrete `Fut`).
                    let event_version = item.version();
                    let decoded = event_codec_ref.decode(item.event_type(), item.payload());
                    async move {
                        let event = decoded.map_err(ProjectionError::EventCodec)?;
                        let next =
                            apply_event(projector_ref, trigger_ref, acc, &event, event_version)
                                .map_err(ProjectionError::Projector)?;
                        if let ProjectionStatus::Committed {
                            state: committed_state,
                            version: committed_version,
                        } = &next
                        {
                            snapshot_store_ref
                                .commit(id_ref, schema_version, *committed_version, committed_state)
                                .await
                                .map_err(ProjectionError::Snapshot)?;
                        }
                        Ok::<
                            ProjectionStatus<P::State>,
                            ProjectionError<P::Error, EC::Error, SS::Error, Sub::Error>,
                        >(next)
                    }
                },
                shutdown,
            )
            .await?;

        // Flush tail: if the fold exited with pending work, commit it once
        // before returning.
        if let ProjectionStatus::Pending {
            state: pending_state,
            version: pending_version,
            ..
        } = &final_status
        {
            snapshot_store
                .commit(&id, schema_version, *pending_version, pending_state)
                .await
                .map_err(ProjectionError::Snapshot)?;
        }
        Ok(())
    }
}
