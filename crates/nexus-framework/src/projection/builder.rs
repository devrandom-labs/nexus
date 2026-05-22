use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};

use nexus_store::state::EveryNEvents;

use super::projection::{Configured, Projection};

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers — compile-time guards for required fields
// ═══════════════════════════════════════════════════════════════════════════

/// Marker: subscription not yet configured. `!Send` prevents `.build()`.
pub struct NeedsSub(PhantomData<*const ()>);
/// Marker: snapshot store not yet configured. `!Send` prevents `.build()`.
pub struct NeedsSnap(PhantomData<*const ()>);
/// Marker: projector not yet configured. `!Send` prevents `.build()`.
pub struct NeedsProj(PhantomData<*const ()>);
/// Marker: event codec not yet configured. `!Send` prevents `.build()`.
pub struct NeedsEvtCodec(PhantomData<*const ()>);

/// Default trigger: persist every event.
const DEFAULT_TRIGGER: EveryNEvents = EveryNEvents(NonZeroU64::MIN);

/// Default state schema version.
const DEFAULT_STATE_SCHEMA_VERSION: NonZeroU32 = NonZeroU32::MIN;

// ═══════════════════════════════════════════════════════════════════════════
// ProjectionBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Typestate builder for [`Projection`].
///
/// Created via [`Projection::builder`]. Required fields must be set
/// before `.build()` becomes available: `subscription`, `snapshot_store`,
/// `projector`, and `event_codec`.
///
/// Optional fields have defaults:
/// - `trigger` -> [`EveryNEvents(1)`](EveryNEvents) (persist every event)
pub struct ProjectionBuilder<Id, Sub, SS, P, EC, Trig> {
    id: Id,
    subscription: Sub,
    snapshot_store: SS,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    schema_version: NonZeroU32,
}

// ── Entry point ─────────────────────────────────────────────────────────

impl<I> Projection<I, NeedsSub, NeedsSnap, NeedsProj, NeedsEvtCodec, EveryNEvents, Configured> {
    /// Start building a new projection.
    #[must_use]
    pub const fn builder(
        id: I,
    ) -> ProjectionBuilder<I, NeedsSub, NeedsSnap, NeedsProj, NeedsEvtCodec, EveryNEvents> {
        ProjectionBuilder {
            id,
            subscription: NeedsSub(PhantomData),
            snapshot_store: NeedsSnap(PhantomData),
            projector: NeedsProj(PhantomData),
            event_codec: NeedsEvtCodec(PhantomData),
            trigger: DEFAULT_TRIGGER,
            schema_version: DEFAULT_STATE_SCHEMA_VERSION,
        }
    }
}

// ── Required setters ────────────────────────────────────────────────────

impl<Id, Sub, SS, P, EC, Trig> ProjectionBuilder<Id, Sub, SS, P, EC, Trig> {
    /// Set the subscription source (event stream).
    #[must_use]
    pub fn subscription<NewSub>(
        self,
        sub: NewSub,
    ) -> ProjectionBuilder<Id, NewSub, SS, P, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: sub,
            snapshot_store: self.snapshot_store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
        }
    }

    /// Set the snapshot store (state + position persistence).
    #[must_use]
    pub fn snapshot_store<NewSS>(
        self,
        store: NewSS,
    ) -> ProjectionBuilder<Id, Sub, NewSS, P, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            snapshot_store: store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
        }
    }

    /// Set the projector (pure event fold function).
    #[must_use]
    pub fn projector<NewP>(self, proj: NewP) -> ProjectionBuilder<Id, Sub, SS, NewP, EC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            snapshot_store: self.snapshot_store,
            projector: proj,
            event_codec: self.event_codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
        }
    }

    /// Set the event codec (deserializes events from the stream).
    #[must_use]
    pub fn event_codec<NewEC>(
        self,
        codec: NewEC,
    ) -> ProjectionBuilder<Id, Sub, SS, P, NewEC, Trig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            snapshot_store: self.snapshot_store,
            projector: self.projector,
            event_codec: codec,
            trigger: self.trigger,
            schema_version: self.schema_version,
        }
    }
}

// ── Optional setters ────────────────────────────────────────────────────

impl<Id, Sub, SS, P, EC, Trig> ProjectionBuilder<Id, Sub, SS, P, EC, Trig> {
    /// Set the projection trigger (when to persist the snapshot).
    ///
    /// Default: [`EveryNEvents(1)`](EveryNEvents) (persist every event).
    #[must_use]
    pub fn trigger<NewTrig>(
        self,
        trigger: NewTrig,
    ) -> ProjectionBuilder<Id, Sub, SS, P, EC, NewTrig> {
        ProjectionBuilder {
            id: self.id,
            subscription: self.subscription,
            snapshot_store: self.snapshot_store,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger,
            schema_version: self.schema_version,
        }
    }

    /// Set the schema version for state invalidation.
    ///
    /// Default: 1. Increment when the projection state shape changes
    /// to trigger re-computation from scratch.
    #[must_use]
    pub const fn state_schema_version(mut self, version: NonZeroU32) -> Self {
        self.schema_version = version;
        self
    }
}

// ── Terminal: .build() ──────────────────────────────────────────────────

impl<Id, Sub, SS, P, EC, Trig> ProjectionBuilder<Id, Sub, SS, P, EC, Trig>
where
    Sub: Send + Sync,
    SS: Send + Sync,
    P: Send + Sync,
    EC: Send + Sync,
{
    /// Build the projection.
    ///
    /// Only available when all required fields are set. The `Send + Sync`
    /// bounds exclude `Needs*` markers (which are `!Send`).
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "generics may have destructors")]
    pub fn build(self) -> Projection<Id, Sub, SS, P, EC, Trig, Configured> {
        Projection::new(
            self.id,
            self.subscription,
            self.snapshot_store,
            self.projector,
            self.event_codec,
            self.trigger,
            self.schema_version,
            Configured,
        )
    }
}
