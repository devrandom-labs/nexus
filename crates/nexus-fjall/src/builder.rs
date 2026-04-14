use crate::encoding::{NEXT_STREAM_ID_KEY, NEXT_STREAM_ID_SIZE, decode_stream_meta};
use crate::error::FjallError;
use crate::partition::{PartitionConfig, point_read_defaults, scan_defaults};
use crate::store::FjallStore;
use fjall::PartitionCreateOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use tokio::sync::Notify;

/// Builder for [`FjallStore`].
///
/// Type parameters `S` and `E` carry the partition configuration
/// closures as concrete types — no `Box<dyn>`, no dynamic dispatch.
/// When unset (defaulting to `()`), the builder passes through the
/// built-in defaults unchanged.
///
/// ```ignore
/// let store = FjallStore::builder("/tmp/my-events")
///     .streams_config(|opts| opts.block_size(4_096))
///     .events_config(|opts| opts.block_size(32_768))
///     .open()?;
/// ```
pub struct FjallStoreBuilder<S = (), E = ()> {
    path: PathBuf,
    streams_config: S,
    events_config: E,
}

impl FjallStoreBuilder {
    /// Create a new builder targeting the given directory.
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: (),
            events_config: (),
        }
    }
}

impl<S, E> FjallStoreBuilder<S, E> {
    /// Customise the `streams` partition options.
    ///
    /// The closure receives a pre-configured `PartitionCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for point-read-heavy metadata lookups (4 KiB blocks, bloom filter).
    #[must_use]
    pub fn streams_config<F>(self, f: F) -> FjallStoreBuilder<F, E>
    where
        F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions,
    {
        FjallStoreBuilder {
            path: self.path,
            streams_config: f,
            events_config: self.events_config,
        }
    }

    /// Customise the `events` partition options.
    ///
    /// The closure receives a pre-configured `PartitionCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for scan-heavy event reads (32 KiB blocks, LZ4 compression).
    #[must_use]
    pub fn events_config<F>(self, f: F) -> FjallStoreBuilder<S, F>
    where
        F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions,
    {
        FjallStoreBuilder {
            path: self.path,
            streams_config: self.streams_config,
            events_config: f,
        }
    }
}

impl<S: PartitionConfig, E: PartitionConfig> FjallStoreBuilder<S, E> {
    /// Open (or create) the fjall database and return a [`FjallStore`].
    ///
    /// On first open the partitions are created with their default
    /// configurations. On reopen, existing partitions are recovered and
    /// the `next_stream_id` counter is rebuilt by scanning `streams` metadata.
    ///
    /// # Errors
    ///
    /// Returns [`FjallError::Io`] if the underlying fjall database cannot
    /// be opened or a partition cannot be created.
    pub fn open(self) -> Result<FjallStore, FjallError> {
        let db = fjall::Config::new(&self.path).open_transactional()?;

        let streams =
            db.open_partition("streams", self.streams_config.apply(point_read_defaults()))?;
        let events = db.open_partition("events", self.events_config.apply(scan_defaults()))?;

        #[cfg(feature = "snapshot")]
        let snapshots = db.open_partition("snapshots", point_read_defaults())?;

        let checkpoints = db.open_partition("checkpoints", point_read_defaults())?;

        // Recover `next_stream_id`: read the persisted counter (O(1)).
        // Falls back to a full scan for databases created before the counter
        // was persisted, then writes the counter for future opens.
        let next_id = match streams.get(NEXT_STREAM_ID_KEY)? {
            Some(bytes) if bytes.len() == NEXT_STREAM_ID_SIZE => {
                u64::from_le_bytes(bytes.as_ref().try_into().map_err(|_| {
                    FjallError::CorruptMeta {
                        stream_id: String::from("__next_stream_id"),
                    }
                })?)
            }
            Some(_) => {
                return Err(FjallError::CorruptMeta {
                    stream_id: String::from("__next_stream_id"),
                });
            }
            None => {
                // Fresh DB or pre-migration: scan to recover, then persist.
                let max_id = streams.inner().iter().try_fold(0u64, |max_id, kv_result| {
                    let kv = kv_result?;
                    let (numeric_id, _version) =
                        decode_stream_meta(&kv.1).map_err(|_| FjallError::CorruptMeta {
                            stream_id: String::from_utf8_lossy(&kv.0).into_owned(),
                        })?;
                    if numeric_id == 0 {
                        return Err(FjallError::CorruptMeta {
                            stream_id: String::from_utf8_lossy(&kv.0).into_owned(),
                        });
                    }
                    Ok(max_id.max(numeric_id))
                })?;
                let next_id = max_id.checked_add(1).ok_or(FjallError::IdSpaceExhausted)?;
                streams.insert(NEXT_STREAM_ID_KEY, next_id.to_le_bytes())?;
                next_id
            }
        };

        Ok(FjallStore {
            db,
            streams,
            events,
            #[cfg(feature = "snapshot")]
            snapshots,
            checkpoints,
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
