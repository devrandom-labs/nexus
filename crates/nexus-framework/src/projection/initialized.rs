use nexus::Id;
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};

use super::error::ProjectionError;
use super::persist::StatePersistence;
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
#[must_use = "an initialized projection does nothing unless .run() is called"]
pub enum Initialized<I, Sub, Ckpt, SP, P: Projector, EC, Trig> {
    /// Checkpoint and state loaded successfully. Will resume from checkpoint.
    Resuming(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>),
    /// Schema mismatch detected. Will replay from beginning of stream.
    Rebuilding(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>),
    /// First run. Will process from beginning of stream.
    Starting(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>),
}

impl<I, Sub, Ckpt, SP, P, EC, Trig> Initialized<I, Sub, Ckpt, SP, P, EC, Trig>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Convenience: run whichever variant was resolved, ignoring the startup mode.
    ///
    /// Equivalent to matching on `self` and calling `.run(shutdown)` on each arm.
    /// Use this when you don't need mode-specific behavior before entering the loop.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] on subscription, codec, projector,
    /// state persistence, or checkpoint failure.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        match self {
            Self::Resuming(prepared) => prepared.run(shutdown).await,
            Self::Rebuilding(prepared) => prepared.run(shutdown).await,
            Self::Starting(prepared) => prepared.run(shutdown).await,
        }
    }
}
