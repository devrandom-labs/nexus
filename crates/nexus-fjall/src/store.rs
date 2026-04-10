use crate::builder::FjallStoreBuilder;
use crate::encoding::{
    decode_stream_meta, encode_event_key, encode_event_value, encode_stream_meta,
};
use crate::error::FjallError;
use crate::stream::FjallStream;
use fjall::Slice;
use nexus::{Id, Version};
use nexus_store::PendingEnvelope;
use nexus_store::ToStreamLabel;
use nexus_store::error::AppendError;
use nexus_store::store::RawEventStore;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fjall-backed event store.
///
/// Holds the transactional keyspace, two partitions (`streams` for
/// stream metadata and `events` for event rows), and a monotonic
/// counter for allocating stream numeric IDs.
///
/// When the `snapshot` feature is enabled, an additional `snapshots`
/// partition is available for storing aggregate snapshots.
///
/// Use [`FjallStore::builder`] to configure and open a store.
pub struct FjallStore {
    pub(crate) db: fjall::TxKeyspace,
    pub(crate) streams: fjall::TxPartitionHandle,
    pub(crate) events: fjall::TxPartitionHandle,
    #[cfg(feature = "snapshot")]
    pub(crate) snapshots: fjall::TxPartitionHandle,
    pub(crate) next_stream_id: AtomicU64,
}

impl FjallStore {
    /// Create a builder for opening a `FjallStore` at the given path.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
    }

    /// Force the internal stream ID counter to a specific value.
    ///
    /// This is intended for testing overflow and exhaustion scenarios.
    /// Production code should never call this.
    #[doc(hidden)]
    pub fn set_next_stream_id_for_testing(&self, value: u64) {
        self.next_stream_id.store(value, Ordering::Relaxed);
    }
}

impl RawEventStore for FjallStore {
    type Error = FjallError;
    type Stream<'a> = FjallStream;

    #[allow(
        clippy::significant_drop_tightening,
        reason = "tx must be held across concurrency check + inserts + commit"
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "transaction scope requires sequential steps in one function"
    )]
    async fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let id_string = id.to_string();
        let expected_raw = expected_version.map_or(0, Version::as_u64);

        // Version check BEFORE empty-batch early return. An empty append
        // with a stale expected_version signals a stale caller — report the
        // conflict even though no data would be written.
        let mut tx = self.db.write_tx();

        let (numeric_id, current_version) = if let Some(meta_bytes) = tx
            .get(&self.streams, &id_string)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
        {
            let (numeric_id, current_version) = decode_stream_meta(&meta_bytes).map_err(|_| {
                AppendError::Store(FjallError::CorruptMeta {
                    stream_id: id_string.clone(),
                })
            })?;
            // Optimistic concurrency check.
            if current_version != expected_raw {
                return Err(AppendError::Conflict {
                    stream_id: id.to_stream_label(),
                    expected: expected_version,
                    actual: Version::new(current_version),
                });
            }
            (numeric_id, current_version)
        } else {
            // New stream: expected_version must be None.
            if expected_version.is_some() {
                return Err(AppendError::Conflict {
                    stream_id: id.to_stream_label(),
                    expected: expected_version,
                    actual: None,
                });
            }
            // Empty batch on a new stream: validate version but don't
            // allocate a numeric ID or create metadata.
            if envelopes.is_empty() {
                return Ok(());
            }
            // Relaxed ordering is safe: write_tx() serializes all writers, so only
            // one thread can reach this code at a time. We use load + checked_add
            // + store instead of fetch_add to prevent silent wrap-around to 0,
            // which would cause numeric ID collision and data corruption.
            let numeric_id = self.next_stream_id.load(Ordering::Relaxed);
            let next_id = numeric_id
                .checked_add(1)
                .ok_or(AppendError::Store(FjallError::IdSpaceExhausted))?;
            self.next_stream_id.store(next_id, Ordering::Relaxed);
            (numeric_id, 0)
        };

        // Empty batch on an existing stream: version was checked, no work to do.
        if envelopes.is_empty() {
            return Ok(());
        }

        // Validate envelope versions are sequential from current_version + 1.
        // Uses checked arithmetic to prevent overflow near u64::MAX.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_version = current_version
                .checked_add(1)
                .and_then(|v| v.checked_add(i_u64))
                .ok_or_else(|| AppendError::Conflict {
                    stream_id: id.to_stream_label(),
                    expected: Version::new(current_version),
                    actual: Some(env.version()),
                })?;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: id.to_stream_label(),
                    expected: Version::new(expected_env_version),
                    actual: Some(env.version()),
                });
            }
        }

        // Write each envelope into the events partition.
        let mut value_buf = Vec::new();
        for env in envelopes {
            let key = encode_event_key(numeric_id, env.version().as_u64());
            encode_event_value(
                &mut value_buf,
                env.schema_version(),
                env.event_type(),
                env.payload(),
            )
            .map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: id_string.clone(),
                    version: env.version().as_u64(),
                    reason: e.to_string(),
                })
            })?;
            tx.insert(&self.events, key, Slice::from(&*value_buf));
        }

        // Update stream meta with new version.
        let new_version = envelopes
            .last()
            .map_or(current_version, |last| last.version().as_u64());

        let meta = encode_stream_meta(numeric_id, new_version);
        tx.insert(&self.streams, &id_string, meta);

        // Atomic cross-partition commit.
        tx.commit()
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?;

        Ok(())
    }

    async fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let id_string = id.to_string();

        // Look up numeric stream ID from streams partition.
        let meta = self.streams.get(&id_string)?;
        let numeric_id = if let Some(meta_bytes) = meta {
            let (numeric_id, _version) =
                decode_stream_meta(&meta_bytes).map_err(|_| FjallError::CorruptMeta {
                    stream_id: id_string.clone(),
                })?;
            numeric_id
        } else {
            // Stream not found: return empty stream.
            return Ok(FjallStream {
                events: vec![],
                pos: 0,
                stream_id: id_string,
                poisoned: false,
                #[cfg(debug_assertions)]
                prev_version: None,
            });
        };

        // Range scan from (numeric_id, from_version) to (numeric_id, u64::MAX).
        let start = encode_event_key(numeric_id, from.as_u64());
        let end = encode_event_key(numeric_id, u64::MAX);

        // Precondition: start key must sort <= end key (same numeric_id).
        debug_assert!(
            start <= end,
            "read_stream precondition: start key must sort <= end key"
        );

        let mut events = Vec::new();
        for kv_result in self.events.inner().range(start..=end) {
            let kv = kv_result?;
            events.push((kv.0, kv.1));
        }

        // Postcondition: events must be sorted by key (fjall guarantees this,
        // but we verify in debug mode).
        #[cfg(debug_assertions)]
        for window in events.windows(2) {
            debug_assert!(
                window[0].0 < window[1].0,
                "read_stream postcondition: events must be strictly sorted by key"
            );
        }

        Ok(FjallStream {
            events,
            pos: 0,
            stream_id: id_string,
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SnapshotStore implementation
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
mod snapshot_impl {
    use super::*;
    use crate::encoding::{
        decode_snapshot_value, decode_stream_meta, encode_snapshot_key, encode_snapshot_value,
    };
    use nexus_store::snapshot::{PendingSnapshot, PersistedSnapshot, SnapshotStore};

    impl SnapshotStore for FjallStore {
        type Error = FjallError;

        async fn load_snapshot(
            &self,
            id: &impl Id,
        ) -> Result<Option<PersistedSnapshot>, FjallError> {
            let id_string = id.to_string();

            // Single snapshot read: look up numeric ID then read snapshot.
            // Both reads use the same point-in-time view (fjall LSM snapshot).
            let meta = self.streams.get(&id_string)?;
            let numeric_id = match meta {
                Some(meta_bytes) => {
                    let (numeric_id, _version) =
                        decode_stream_meta(&meta_bytes).map_err(|_| FjallError::CorruptMeta {
                            stream_id: id_string.clone(),
                        })?;
                    numeric_id
                }
                None => return Ok(None),
            };

            let key = encode_snapshot_key(numeric_id);
            let value = self.snapshots.get(key)?;
            match value {
                Some(bytes) => {
                    let (schema_version_raw, version_raw, payload) = decode_snapshot_value(&bytes)
                        .map_err(|_| FjallError::CorruptValue {
                            stream_id: id_string,
                            version: None,
                        })?;
                    let version =
                        Version::new(version_raw).ok_or_else(|| FjallError::CorruptValue {
                            stream_id: id.to_string(),
                            version: None,
                        })?;
                    let schema_version =
                        std::num::NonZeroU32::new(schema_version_raw).ok_or_else(|| {
                            FjallError::CorruptValue {
                                stream_id: id.to_string(),
                                version: Some(version_raw),
                            }
                        })?;
                    let snap = PersistedSnapshot::new(version, schema_version, payload.to_vec());
                    Ok(Some(snap))
                }
                None => Ok(None),
            }
        }

        async fn save_snapshot(
            &self,
            id: &impl Id,
            snapshot: &PendingSnapshot,
        ) -> Result<(), FjallError> {
            let id_string = id.to_string();

            // Atomic: look up numeric ID + write snapshot in one transaction.
            let mut tx = self.db.write_tx();

            let numeric_id = match tx
                .get(&self.streams, &id_string)
                .map_err(|e| FjallError::Io(e))?
            {
                Some(meta_bytes) => {
                    let (numeric_id, _version) =
                        decode_stream_meta(&meta_bytes).map_err(|_| FjallError::CorruptMeta {
                            stream_id: id_string.clone(),
                        })?;
                    numeric_id
                }
                None => {
                    // No stream exists — can't snapshot an aggregate with no events.
                    return Ok(());
                }
            };

            let key = encode_snapshot_key(numeric_id);
            let mut buf = Vec::new();
            encode_snapshot_value(
                &mut buf,
                snapshot.schema_version().get(),
                snapshot.version().as_u64(),
                snapshot.payload(),
            );
            tx.insert(&self.snapshots, key, fjall::Slice::from(&*buf));

            tx.commit().map_err(FjallError::Io)?;
            Ok(())
        }

        async fn delete_snapshot(&self, id: &impl Id) -> Result<(), FjallError> {
            let id_string = id.to_string();

            let mut tx = self.db.write_tx();

            let numeric_id = match tx
                .get(&self.streams, &id_string)
                .map_err(|e| FjallError::Io(e))?
            {
                Some(meta_bytes) => {
                    let (numeric_id, _version) =
                        decode_stream_meta(&meta_bytes).map_err(|_| FjallError::CorruptMeta {
                            stream_id: id_string.clone(),
                        })?;
                    numeric_id
                }
                None => return Ok(()),
            };

            let key = encode_snapshot_key(numeric_id);
            tx.remove(&self.snapshots, key);
            tx.commit().map_err(FjallError::Io)?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::store::EventStream;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct TestId(String);

    impl std::fmt::Display for TestId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl nexus::Id for TestId {}

    fn tid(s: &str) -> TestId {
        TestId(s.to_owned())
    }

    fn make_envelope(
        version: u64,
        event_type: &'static str,
        payload: &[u8],
    ) -> PendingEnvelope<()> {
        pending_envelope(Version::new(version).expect("test version must be > 0"))
            .event_type(event_type)
            .payload(payload.to_vec())
            .build_without_metadata()
    }

    fn temp_store() -> (FjallStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        (store, dir)
    }

    // ---- append tests ----

    #[tokio::test]
    async fn append_to_new_stream() {
        let (store, _dir) = temp_store();
        let env = make_envelope(1, "Created", b"payload");

        let result = store.append(&tid("stream-1"), None, &[env]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_multiple_events() {
        let (store, _dir) = temp_store();
        let envs = vec![
            make_envelope(1, "Created", b"p1"),
            make_envelope(2, "Updated", b"p2"),
            make_envelope(3, "Updated", b"p3"),
        ];

        let result = store.append(&tid("stream-1"), None, &envs).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_detects_version_conflict() {
        let (store, _dir) = temp_store();
        let env1 = make_envelope(1, "Created", b"p1");
        store.append(&tid("stream-1"), None, &[env1]).await.unwrap();

        // Try appending with wrong expected version (None instead of Some(1)).
        let env2 = make_envelope(1, "Duplicate", b"p2");
        let result = store.append(&tid("stream-1"), None, &[env2]).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, None);
                assert_eq!(actual, Version::new(1));
            }
            other @ AppendError::Store(_) => panic!("expected Conflict, got: {other}"),
        }
    }

    #[tokio::test]
    async fn append_to_nonexistent_stream_with_nonzero_version_fails() {
        let (store, _dir) = temp_store();
        let env = make_envelope(6, "Created", b"payload");

        let result = store
            .append(&tid("stream-1"), Version::new(5), &[env])
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, Version::new(5));
                assert_eq!(actual, None);
            }
            other @ AppendError::Store(_) => panic!("expected Conflict, got: {other}"),
        }
    }

    // ---- read_stream tests ----

    #[tokio::test]
    async fn read_after_append_returns_events() {
        let (store, _dir) = temp_store();
        let envs = vec![
            make_envelope(1, "Created", b"p1"),
            make_envelope(2, "Updated", b"p2"),
        ];
        store.append(&tid("stream-1"), None, &envs).await.unwrap();

        let mut stream = store
            .read_stream(&tid("stream-1"), Version::new(1).unwrap())
            .await
            .unwrap();

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.version(), Version::new(1).unwrap());
            assert_eq!(env.event_type(), "Created");
            assert_eq!(env.payload(), b"p1");
        }

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.version(), Version::new(2).unwrap());
            assert_eq!(env.event_type(), "Updated");
            assert_eq!(env.payload(), b"p2");
        }

        assert!(stream.next().await.is_none());
    }

    // ---- version tracking tests ----

    #[tokio::test]
    async fn version_tracks_across_appends() {
        let (store, _dir) = temp_store();
        store
            .append(&tid("s"), None, &[make_envelope(1, "A", b"")])
            .await
            .unwrap();
        store
            .append(&tid("s"), Version::new(1), &[make_envelope(2, "B", b"")])
            .await
            .unwrap();

        let mut stream = store
            .read_stream(&tid("s"), Version::new(1).unwrap())
            .await
            .unwrap();
        {
            let e1 = stream.next().await.unwrap().unwrap();
            assert_eq!(e1.version().as_u64(), 1);
        }
        {
            let e2 = stream.next().await.unwrap().unwrap();
            assert_eq!(e2.version().as_u64(), 2);
        }
        assert!(stream.next().await.is_none());
    }
}
