use std::future::Future;

use nexus::{Aggregate, AggregateRoot, Version};

use crate::error::StoreError;

/// Internal trait for replaying events from a given starting point.
///
/// Both `EventStore` and `ZeroCopyEventStore` implement this to share
/// replay logic with `Snapshotting`. Not public API.
pub trait ReplayFrom<A: Aggregate>: Send + Sync {
    /// Replay events starting from `from` version (inclusive) into `root`.
    ///
    /// Returns the updated aggregate with all events applied.
    fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> impl Future<Output = Result<AggregateRoot<A>, StoreError>> + Send;
}
