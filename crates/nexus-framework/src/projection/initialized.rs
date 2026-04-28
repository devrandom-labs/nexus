use nexus_store::projection::Projector;

use super::prepared::{PreparedProjection, Rebuilding, Resuming, Starting};

// ═══════════════════════════════════════════════════════════════════════════
// Initialized — the return type of ProjectionRunner::initialize()
// ═══════════════════════════════════════════════════════════════════════════

/// The result of [`ProjectionRunner::initialize`](super::ProjectionRunner).
///
/// Forces the supervisor to handle all three startup outcomes:
/// - [`Resuming`] — loaded state, can [`force_rebuild`](PreparedProjection::force_rebuild) or run
/// - [`Rebuilding`] — schema mismatch, can run or drop to abort
/// - [`Starting`] — first run, can run or drop to abort
pub enum Initialized<I, Sub, Ckpt, SP, P: Projector, EC, Trig> {
    /// Checkpoint and state loaded successfully. Will resume from checkpoint.
    Resuming(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>),
    /// Schema mismatch detected. Will replay from beginning of stream.
    Rebuilding(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>),
    /// First run. Will process from beginning of stream.
    Starting(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>),
}
