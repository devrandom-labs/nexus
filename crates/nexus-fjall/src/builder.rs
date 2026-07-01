use crate::error::FjallError;
use crate::partition::{AllIndex, KeyspaceConfig, Partitions, point_read_defaults, scan_defaults};
use crate::store::FjallStore;
use fjall::KeyspaceCreateOptions;
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
    all_index: AllIndex,
}

impl FjallStoreBuilder {
    /// Create a new builder targeting the given directory.
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: (),
            events_config: (),
            all_index: AllIndex::default(),
        }
    }
}

impl<S, E> FjallStoreBuilder<S, E> {
    /// Choose whether the store maintains the `$all` cross-stream index.
    ///
    /// Defaults to [`AllIndex::Denormalized`] (a read-optimized `$all`). Set
    /// [`AllIndex::Disabled`] for produce-and-sync `IoT` devices that never read
    /// `$all` locally — `append` then skips the second write, reclaiming ~27%–2×
    /// of write/storage, and `read_all` returns
    /// [`FjallError::AllIndexDisabled`](crate::FjallError::AllIndexDisabled).
    #[must_use]
    pub const fn all_index(mut self, mode: AllIndex) -> Self {
        self.all_index = mode;
        self
    }

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
            all_index: self.all_index,
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
            all_index: self.all_index,
        }
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
        let events_global = db.keyspace("events_global", scan_defaults)?;

        #[cfg(feature = "snapshot")]
        let snapshots = db.keyspace("snapshots", point_read_defaults)?;

        let global = db.keyspace("global", point_read_defaults)?;

        Ok(FjallStore {
            db,
            partitions: Partitions::new(
                streams,
                events,
                events_global,
                global,
                #[cfg(feature = "snapshot")]
                snapshots,
            )
            .with_all_index(self.all_index),
            notifiers: StreamNotifiers::new(),
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
