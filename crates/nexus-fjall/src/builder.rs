use crate::error::FjallError;
use crate::partition::{KeyspaceConfig, point_read_defaults, scan_defaults};
use crate::store::FjallStore;
use fjall::KeyspaceCreateOptions;
use nexus_store::batch::BatchSize;
use nexus_store::notify::StreamNotifiers;
use std::path::{Path, PathBuf};

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
    batch_size: BatchSize,
}

impl FjallStoreBuilder {
    /// Create a new builder targeting the given directory.
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: (),
            events_config: (),
            batch_size: BatchSize::DEFAULT,
        }
    }
}

impl<S, E> FjallStoreBuilder<S, E> {
    /// Customise the `streams` partition options.
    ///
    /// The closure receives a pre-configured `KeyspaceCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for point-read-heavy metadata lookups (4 KiB blocks, bloom filter).
    #[must_use]
    pub fn streams_config<F>(self, f: F) -> FjallStoreBuilder<F, E>
    where
        F: FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions,
    {
        FjallStoreBuilder {
            path: self.path,
            streams_config: f,
            events_config: self.events_config,
            batch_size: self.batch_size,
        }
    }

    /// Customise the `events` partition options.
    ///
    /// The closure receives a pre-configured `KeyspaceCreateOptions` and
    /// should return the (possibly modified) options. Defaults are tuned
    /// for scan-heavy event reads (32 KiB blocks, LZ4 compression).
    #[must_use]
    pub fn events_config<F>(self, f: F) -> FjallStoreBuilder<S, F>
    where
        F: FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions,
    {
        FjallStoreBuilder {
            path: self.path,
            streams_config: self.streams_config,
            events_config: f,
            batch_size: self.batch_size,
        }
    }

    /// Set the read / refill batch size — the maximum number of event rows
    /// held in memory at once by a read stream or subscription refill.
    ///
    /// Out-of-range values are rejected when constructing the
    /// [`BatchSize`](nexus_store::batch::BatchSize), so this setter is
    /// infallible. Defaults to [`BatchSize::DEFAULT`].
    #[must_use]
    pub const fn batch_size(mut self, batch_size: BatchSize) -> Self {
        self.batch_size = batch_size;
        self
    }
}

impl<S: KeyspaceConfig, E: KeyspaceConfig> FjallStoreBuilder<S, E> {
    /// Open (or create) the fjall database and return a [`FjallStore`].
    ///
    /// On first open the partitions are created with their default
    /// configurations. On reopen, existing partitions are recovered
    /// automatically by fjall.
    ///
    /// # Errors
    ///
    /// Returns [`FjallError::Io`] if the underlying fjall database cannot
    /// be opened or a partition cannot be created.
    pub fn open(self) -> Result<FjallStore, FjallError> {
        let db = fjall::SingleWriterTxDatabase::builder(&self.path).open()?;

        let streams_opts = self.streams_config.apply(point_read_defaults());
        let streams = db.keyspace("streams", || streams_opts)?;
        let events_opts = self.events_config.apply(scan_defaults());
        let events = db.keyspace("events", || events_opts)?;

        #[cfg(feature = "snapshot")]
        let snapshots = db.keyspace("snapshots", point_read_defaults)?;

        let global = db.keyspace("global", point_read_defaults)?;

        Ok(FjallStore {
            db,
            streams,
            events,
            #[cfg(feature = "snapshot")]
            snapshots,
            global,
            notifiers: StreamNotifiers::new(),
            batch_size: self.batch_size,
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
            drop(store);
        }

        // Second open — recovers from existing data.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            drop(store);
        }
    }
}
