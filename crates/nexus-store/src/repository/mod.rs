mod builder;
mod event_store;
#[allow(
    clippy::redundant_pub_crate,
    reason = "replay is crate-internal; pub(crate) documents intent even though parent is private"
)]
pub(crate) mod replay;
#[allow(
    clippy::module_inception,
    reason = "Repository trait is the primary public type of the repository module"
)]
mod repository;
#[cfg(feature = "snapshot")]
mod snapshot;
mod zero_copy;

#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
pub use builder::{NeedsCodec, NoSnapshot, RepositoryBuilder};
pub use event_store::EventStore;
pub use repository::Repository;
#[cfg(feature = "snapshot")]
pub use snapshot::Snapshotting;
pub use zero_copy::ZeroCopyEventStore;
