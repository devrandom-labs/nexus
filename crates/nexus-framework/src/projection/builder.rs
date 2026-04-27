use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};

use nexus_store::projection::EveryNEvents;

use super::persist::{NoStatePersistence, WithStatePersistence};
use super::runner::ProjectionRunner;

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers — compile-time guards for required fields
// ═══════════════════════════════════════════════════════════════════════════

/// Marker: subscription not yet configured. `!Send` prevents `.build()`.
pub struct NeedsSub(PhantomData<*const ()>);
/// Marker: checkpoint store not yet configured. `!Send` prevents `.build()`.
pub struct NeedsCkpt(PhantomData<*const ()>);
/// Marker: projector not yet configured. `!Send` prevents `.build()`.
pub struct NeedsProj(PhantomData<*const ()>);
/// Marker: event codec not yet configured. `!Send` prevents `.build()`.
pub struct NeedsEvtCodec(PhantomData<*const ()>);

/// Default trigger: checkpoint every event.
const DEFAULT_TRIGGER: EveryNEvents = EveryNEvents(NonZeroU64::MIN);

/// Default state schema version.
const DEFAULT_STATE_SCHEMA_VERSION: NonZeroU32 = NonZeroU32::MIN;

// ═══════════════════════════════════════════════════════════════════════════
// ProjectionRunnerBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Typestate builder for [`ProjectionRunner`].
///
/// Created via [`ProjectionRunner::builder`]. Required fields must be set
/// before `.build()` becomes available: `subscription`, `checkpoint`,
/// `projector`, and `event_codec`.
///
/// Optional fields have defaults:
/// - `state_persistence` -> [`NoStatePersistence`] (state not persisted)
/// - `trigger` -> [`EveryNEvents(1)`](EveryNEvents) (checkpoint every event)
pub struct ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    id: Id,
    subscription: Sub,
    checkpoint: Ckpt,
    state_persistence: SP,
    projector: P,
    event_codec: EC,
    trigger: Trig,
}

// ── Entry point ─────────────────────────────────────────────────────────

impl<I>
    ProjectionRunner<
        I,
        NeedsSub,
        NeedsCkpt,
        NoStatePersistence,
        NeedsProj,
        NeedsEvtCodec,
        EveryNEvents,
    >
{
    /// Start building a new projection runner.
    #[must_use]
    pub const fn builder(
        id: I,
    ) -> ProjectionRunnerBuilder<
        I,
        NeedsSub,
        NeedsCkpt,
        NoStatePersistence,
        NeedsProj,
        NeedsEvtCodec,
        EveryNEvents,
    > {
        ProjectionRunnerBuilder {
            id,
            subscription: NeedsSub(PhantomData),
            checkpoint: NeedsCkpt(PhantomData),
            state_persistence: NoStatePersistence,
            projector: NeedsProj(PhantomData),
            event_codec: NeedsEvtCodec(PhantomData),
            trigger: DEFAULT_TRIGGER,
        }
    }
}

// ── Required setters ────────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    /// Set the subscription source (event stream).
    #[must_use]
    pub fn subscription<NewSub>(
        self,
        sub: NewSub,
    ) -> ProjectionRunnerBuilder<Id, NewSub, Ckpt, SP, P, EC, Trig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: sub,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
        }
    }

    /// Set the checkpoint store (resume position persistence).
    #[must_use]
    pub fn checkpoint<NewCkpt>(
        self,
        ckpt: NewCkpt,
    ) -> ProjectionRunnerBuilder<Id, Sub, NewCkpt, SP, P, EC, Trig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: ckpt,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
        }
    }

    /// Set the projector (pure event fold function).
    #[must_use]
    pub fn projector<NewP>(
        self,
        proj: NewP,
    ) -> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, NewP, EC, Trig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: proj,
            event_codec: self.event_codec,
            trigger: self.trigger,
        }
    }

    /// Set the event codec (deserializes events from the stream).
    #[must_use]
    pub fn event_codec<NewEC>(
        self,
        codec: NewEC,
    ) -> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, NewEC, Trig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: codec,
            trigger: self.trigger,
        }
    }
}

// ── Optional setters ────────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    /// Enable state persistence with a store and codec.
    ///
    /// When not called, state persistence is disabled — the runner only
    /// checkpoints progress, it doesn't persist the folded state.
    #[must_use]
    pub fn state_store<SS, SC>(
        self,
        store: SS,
        codec: SC,
    ) -> ProjectionRunnerBuilder<Id, Sub, Ckpt, WithStatePersistence<SS, SC>, P, EC, Trig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: WithStatePersistence {
                store,
                codec,
                schema_version: DEFAULT_STATE_SCHEMA_VERSION,
            },
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
        }
    }

    /// Set the projection trigger (when to persist state + checkpoint).
    ///
    /// Default: [`EveryNEvents(1)`](EveryNEvents) (checkpoint every event).
    #[must_use]
    pub fn trigger<NewTrig>(
        self,
        trigger: NewTrig,
    ) -> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, EC, NewTrig> {
        ProjectionRunnerBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger,
        }
    }
}

// ── Schema version setter (only when state store is configured) ─────────

impl<Id, Sub, Ckpt, SS, SC, P, EC, Trig>
    ProjectionRunnerBuilder<Id, Sub, Ckpt, WithStatePersistence<SS, SC>, P, EC, Trig>
{
    /// Set the schema version for state invalidation.
    ///
    /// Default: 1. Increment when the projection state shape changes
    /// to trigger re-computation from scratch.
    #[must_use]
    pub const fn state_schema_version(mut self, version: NonZeroU32) -> Self {
        self.state_persistence.schema_version = version;
        self
    }
}

// ── Terminal: .build() ──────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunnerBuilder<Id, Sub, Ckpt, SP, P, EC, Trig>
where
    Sub: Send + Sync,
    Ckpt: Send + Sync,
    P: Send + Sync,
    EC: Send + Sync,
{
    /// Build the projection runner.
    ///
    /// Only available when all required fields are set. The `Send + Sync`
    /// bounds exclude `Needs*` markers (which are `!Send`).
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "generics may have destructors")]
    pub fn build(self) -> ProjectionRunner<Id, Sub, Ckpt, SP, P, EC, Trig> {
        ProjectionRunner {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
        }
    }
}
