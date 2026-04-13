use crate::encoding::decode_stream_meta;
use crate::error::FjallError;
use crate::store::FjallStore;
use fjall::{CompressionType, PartitionCreateOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use tokio::sync::Notify;

/// Builder for [`FjallStore`].
///
/// Allows customising the fjall `PartitionCreateOptions` for the `streams`
/// and `events` partitions before opening the database.
///
/// ```ignore
/// let store = FjallStore::builder("/tmp/my-events")
///     .streams_config(|opts| opts.block_size(4_096))
///     .events_config(|opts| opts.block_size(32_768))
///     .open()?;
/// ```
pub struct FjallStoreBuilder {
    path: PathBuf,
    streams_config:
        Option<Box<dyn FnOnce(PartitionCreateOptions) -> PartitionCreateOptions + Send>>,
    events_config: Option<Box<dyn FnOnce(PartitionCreateOptions) -> PartitionCreateOptions + Send>>,
}

impl FjallStoreBuilder {
    /// Create a new builder targeting the given directory.
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: None,
            events_config: None,
        }
    }

    /// Customise the `streams` partition options.
    ///
    /// The closure receives a pre-configured `PartitionCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for point-read-heavy metadata lookups (4 KiB blocks, bloom filter).
    #[must_use]
    pub fn streams_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions + Send + 'static,
    {
        self.streams_config = Some(Box::new(f));
        self
    }

    /// Customise the `events` partition options.
    ///
    /// The closure receives a pre-configured `PartitionCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for scan-heavy event reads (32 KiB blocks, LZ4 compression).
    #[must_use]
    pub fn events_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions + Send + 'static,
    {
        self.events_config = Some(Box::new(f));
        self
    }

    /// Open (or create) the fjall database and return a [`FjallStore`].
    ///
    /// On first open the `streams` and `events` partitions are created.
    /// On reopen, existing partitions are recovered and the `next_stream_id`
    /// counter is rebuilt by scanning `streams` metadata.
    ///
    /// # Errors
    ///
    /// Returns [`FjallError::Io`] if the underlying fjall database cannot
    /// be opened or a partition cannot be created.
    pub fn open(self) -> Result<FjallStore, FjallError> {
        let db = fjall::Config::new(&self.path).open_transactional()?;

        // --- streams partition: point-read-optimised defaults ---
        // 4 KiB blocks for small metadata lookups, 15-bit bloom filter (~0.1% FPR)
        // to minimise unnecessary I/O on point reads.
        let streams_defaults = PartitionCreateOptions::default()
            .block_size(4_096)
            .bloom_filter_bits(Some(15));
        let streams_opts = match self.streams_config {
            Some(f) => f(streams_defaults),
            None => streams_defaults,
        };
        let streams = db.open_partition("streams", streams_opts)?;

        // --- events partition: scan-optimised defaults ---
        // 32 KiB blocks for efficient sequential scans, LZ4 compression
        // for reduced disk I/O on large event streams.
        let events_defaults = PartitionCreateOptions::default()
            .block_size(32_768)
            .compression(CompressionType::Lz4);
        let events_opts = match self.events_config {
            Some(f) => f(events_defaults),
            None => events_defaults,
        };
        let events = db.open_partition("events", events_opts)?;

        // --- snapshots partition: point-read-optimised (like streams) ---
        #[cfg(feature = "snapshot")]
        let snapshots = {
            let snapshots_defaults = PartitionCreateOptions::default()
                .block_size(4_096)
                .bloom_filter_bits(Some(15));
            db.open_partition("snapshots", snapshots_defaults)?
        };

        // Recover `next_stream_id` by scanning all stream metadata entries
        // and finding the maximum numeric_id.
        let mut max_id: u64 = 0;
        for kv_result in streams.inner().iter() {
            let kv = kv_result?;
            let (numeric_id, _version) =
                decode_stream_meta(&kv.1).map_err(|_| FjallError::CorruptMeta {
                    stream_id: String::from_utf8_lossy(&kv.0).into_owned(),
                })?;
            // Numeric ID 0 is reserved as "no stream" sentinel. If we see it
            // in the database, the store is corrupt — reject early rather than
            // silently reallocating ID 0 to a new stream.
            if numeric_id == 0 {
                return Err(FjallError::CorruptMeta {
                    stream_id: String::from_utf8_lossy(&kv.0).into_owned(),
                });
            }
            if numeric_id > max_id {
                max_id = numeric_id;
            }
        }

        // Next assignable ID is max + 1 (or 1 if no streams exist yet,
        // since 0 is reserved as "no stream"). Uses checked_add to prevent
        // silent wrap-around to 0 when the full ID space is exhausted.
        let next_id = max_id.checked_add(1).ok_or(FjallError::IdSpaceExhausted)?;

        Ok(FjallStore {
            db,
            streams,
            events,
            #[cfg(feature = "snapshot")]
            snapshots,
            next_stream_id: AtomicU64::new(next_id),
            notify: Notify::new(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    #[test]
    fn opens_and_closes_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        drop(store);
    }

    #[test]
    fn reopens_existing_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db");

        // First open — creates partitions.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            // next_stream_id should start at 1 for an empty database.
            assert_eq!(
                store
                    .next_stream_id
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
            drop(store);
        }

        // Second open — recovers from existing data.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            assert_eq!(
                store
                    .next_stream_id
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
            drop(store);
        }
    }
}
