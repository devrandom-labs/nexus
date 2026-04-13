mod builder;
mod event_store;
pub mod replay;
#[allow(
    clippy::module_inception,
    reason = "Repository trait is the primary public type of the repository module"
)]
mod repository;
mod zero_copy;

#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
pub use builder::{NeedsCodec, NoSnapshot, RepositoryBuilder};
pub use event_store::EventStore;
pub use repository::Repository;
pub use zero_copy::ZeroCopyEventStore;
