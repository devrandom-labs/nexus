mod builder;
mod event_store;
mod raw;
pub(crate) mod replay;
mod repository;
#[cfg(feature = "snapshot")]
mod snapshotting;
#[allow(
    clippy::module_inception,
    reason = "Store<S> is the primary public type of the store module"
)]
mod store;
mod stream;
mod zero_copy_event_store;

#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
pub use builder::{NeedsCodec, NoSnapshot, RepositoryBuilder};
pub use event_store::EventStore;
pub use raw::RawEventStore;
pub use repository::Repository;
#[cfg(feature = "snapshot")]
pub use snapshotting::Snapshotting;
pub use store::Store;
pub use stream::{EventStream, EventStreamExt};
pub use zero_copy_event_store::ZeroCopyEventStore;
