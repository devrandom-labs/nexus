use std::future::Future;

use nexus::{Id, Version};

/// Persists subscription positions for resume-after-restart.
///
/// A checkpoint records how far a subscription has processed events
/// in a stream. On restart, the subscriber loads its checkpoint and
/// passes it as `from` to [`Subscription::subscribe`].
///
/// # Contract
///
/// - `load` returns `None` for unknown subscription IDs (not an error).
/// - `save` overwrites the previous checkpoint for the given ID.
/// - Implementations must be durable — a saved checkpoint must survive
///   process restart.
pub trait CheckpointStore {
    /// The error type for checkpoint operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Load the last saved checkpoint for a subscription.
    ///
    /// Returns `None` if no checkpoint has been saved for this ID.
    fn load(
        &self,
        subscription_id: &impl Id,
    ) -> impl Future<Output = Result<Option<Version>, Self::Error>> + Send + '_;

    /// Save (overwrite) the checkpoint for a subscription.
    fn save(
        &self,
        subscription_id: &impl Id,
        version: Version,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + '_;
}
