use std::marker::PhantomData;

use nexus::Version;
use nexus_store::projection::Projector;

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers — encode the startup decision at the type level
// ═══════════════════════════════════════════════════════════════════════════

/// Startup decision: resuming from a checkpoint with loaded state.
pub struct Resuming;

/// Startup decision: schema mismatch detected, replaying from beginning.
pub struct Rebuilding;

/// Startup decision: first run, processing from beginning.
pub struct Starting;

// ═══════════════════════════════════════════════════════════════════════════
// PreparedProjection
// ═══════════════════════════════════════════════════════════════════════════

/// A projection that has completed startup and is ready to run.
///
/// Created by [`ProjectionRunner::initialize`](super::ProjectionRunner).
/// The `Mode` parameter encodes the startup decision as a typestate:
/// - [`Resuming`] — loaded state, will resume from checkpoint
/// - [`Rebuilding`] — schema mismatch, will replay from beginning
/// - [`Starting`] — first run, will process from beginning
///
/// Call [`run`](PreparedProjection::run) to enter the event loop, or inspect
/// the resolved state via mode-specific accessors before running.
#[expect(
    dead_code,
    reason = "fields consumed by run() and accessors in upcoming tasks"
)]
pub struct PreparedProjection<I, Sub, Ckpt, SP, P: Projector, EC, Trig, Mode> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) state: P::State,
    pub(crate) resume_from: Option<Version>,
    pub(crate) _mode: PhantomData<Mode>,
}
