use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};

use nexus_store::state::EveryNEvents;

use super::projection::{Configured, Projection};

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
// ProjectionBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Typestate builder for [`Projection`].
///
/// Created via [`Projection::builder`]. Required fields must be set
/// before `.build()` becomes available: `subscription`, `checkpoint`,
/// `projector`, and `event_codec`.
///
/// Optional fields have defaults:
/// - `state_store` -> `()` (state not persisted)
/// - `trigger` -> [`EveryNEvents(1)`](EveryNEvents) (checkpoint every event)
pub struct ProjectionBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    id: Id,
    subscription: Sub,
    checkpoint: Ckpt,
    state_store: SP,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    schema_version: NonZeroU32,
    persists_state: bool,
}

// ── Entry point ─────────────────────────────────────────────────────────

impl<I> Projection<I, NeedsSub, NeedsCkpt, (), NeedsProj, NeedsEvtCodec, EveryNEvents, Configured> {
    /// Start building a new projection.
    #[must_use]
    pub const fn builder(
        id: I,
    ) -> ProjectionBuilder<I, NeedsSub, NeedsCkpt, (), NeedsProj, NeedsEvtCodec, EveryNEvents> {
        ProjectionBuilder {
            id,
            subscription: NeedsSub(PhantomData),
            checkpoint: NeedsCkpt(PhantomData),
            state_store: (),
            projector: NeedsProj(PhantomData),
            event_codec: NeedsEvtCodec(PhantomData),
            trigger: DEFAULT_TRIGGER,
            schema_version: DEFAULT_STATE_SCHEMA_VERSION,
            persists_state: false,
        }
    }
}

// ── Required setters ────────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    /// Set the subscription source (event stream).
    #[must_use]
    pub fn subscription<NewSub>(
        self,
        sub: NewSub,
    ) -> ProjectionBuilder<Id, NewSub, Ckpt, SP, P, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: sub,
            checkpoint: self.checkpoint,
            state_store: self.state_store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
            persists_state: self.persists_state,
        }
    }

    /// Set the checkpoint store (resume position persistence).
    #[must_use]
    pub fn checkpoint<NewCkpt>(
        self,
        ckpt: NewCkpt,
    ) -> ProjectionBuilder<Id, Sub, NewCkpt, SP, P, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: ckpt,
            state_store: self.state_store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
            persists_state: self.persists_state,
        }
    }

    /// Set the projector (pure event fold function).
    #[must_use]
    pub fn projector<NewP>(
        self,
        proj: NewP,
    ) -> ProjectionBuilder<Id, Sub, Ckpt, SP, NewP, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_store: self.state_store,
            projector: proj,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
            persists_state: self.persists_state,
        }
    }

    /// Set the event codec (deserializes events from the stream).
    #[must_use]
    pub fn event_codec<NewEC>(
        self,
        codec: NewEC,
    ) -> ProjectionBuilder<Id, Sub, Ckpt, SP, P, NewEC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_store: self.state_store,
            projector: self.projector,
            event_codec: codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
            persists_state: self.persists_state,
        }
    }
}

// ── Optional setters ────────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionBuilder<Id, Sub, Ckpt, SP, P, EC, Trig> {
    /// Enable state persistence with a typed state store.
    ///
    /// When not called, state persistence is disabled — the projection only
    /// checkpoints progress, it doesn't persist the folded state.
    #[must_use]
    pub fn state_store<NewSP>(
        self,
        store: NewSP,
    ) -> ProjectionBuilder<Id, Sub, Ckpt, NewSP, P, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_store: store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
            persists_state: true,
        }
    }

    /// Set the projection trigger (when to persist state + checkpoint).
    ///
    /// Default: [`EveryNEvents(1)`](EveryNEvents) (checkpoint every event).
    #[must_use]
    pub fn trigger<NewTrig>(
        self,
        trigger: NewTrig,
    ) -> ProjectionBuilder<Id, Sub, Ckpt, SP, P, EC, NewTrig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_store: self.state_store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger,
            schema_version: self.schema_version,
            persists_state: self.persists_state,
        }
    }

    /// Set the schema version for state invalidation.
    ///
    /// Default: 1. Increment when the projection state shape changes
    /// to trigger re-computation from scratch.
    ///
    /// Only meaningful when a state store is configured via
    /// [`.state_store()`](Self::state_store).
    #[must_use]
    pub const fn state_schema_version(mut self, version: NonZeroU32) -> Self {
        self.schema_version = version;
        self
    }
}

// ── Terminal: .build() ──────────────────────────────────────────────────

impl<Id, Sub, Ckpt, SP, P, EC, Trig> ProjectionBuilder<Id, Sub, Ckpt, SP, P, EC, Trig>
where
    Sub: Send + Sync,
    Ckpt: Send + Sync,
    P: Send + Sync,
    EC: Send + Sync,
{
    /// Build the projection.
    ///
    /// Only available when all required fields are set. The `Send + Sync`
    /// bounds exclude `Needs*` markers (which are `!Send`).
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "generics may have destructors")]
    pub fn build(self) -> Projection<Id, Sub, Ckpt, SP, P, EC, Trig, Configured> {
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
            Configured,
        )
    }
}
