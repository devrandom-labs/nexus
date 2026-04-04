use crate::builder::FjallStoreBuilder;
use std::path::Path;
use std::sync::atomic::AtomicU64;

/// Fjall-backed event store.
///
/// Holds the transactional keyspace, two partitions (`streams` for
/// stream metadata and `events` for event rows), and a monotonic
/// counter for allocating stream numeric IDs.
///
/// Use [`FjallStore::builder`] to configure and open a store.
#[allow(dead_code, reason = "fields used by RawEventStore impl (Task 6/7)")]
pub struct FjallStore {
    pub(crate) db: fjall::TxKeyspace,
    pub(crate) streams: fjall::TxPartitionHandle,
    pub(crate) events: fjall::TxPartitionHandle,
    pub(crate) next_stream_id: AtomicU64,
}

impl FjallStore {
    /// Create a builder for opening a `FjallStore` at the given path.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
    }
}
