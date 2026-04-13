mod raw;
mod repository;
#[cfg(feature = "snapshot")]
mod snapshot;
#[allow(
    clippy::module_inception,
    reason = "Store<S> is the primary public type of the store module"
)]
mod store;
mod stream;
mod subscription;

pub use raw::RawEventStore;
#[cfg(feature = "snapshot")]
pub use repository::WithSnapshot;
pub use repository::{
    EventStore, NeedsCodec, NoSnapshot, Repository, RepositoryBuilder, ZeroCopyEventStore,
};
#[cfg(feature = "snapshot")]
pub use snapshot::Snapshotting;
pub use store::Store;
pub use stream::{EventStream, EventStreamExt};
pub use subscription::{CheckpointStore, Subscription};
