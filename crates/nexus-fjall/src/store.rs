use crate::builder::FjallStoreBuilder;
use crate::encoding::{
    decode_stream_version, encode_event_key, encode_event_value, encode_stream_version,
};
use crate::error::FjallError;
use crate::stream::FjallStream;
use crate::subscription_stream::{FjallSubscriptionStream, OwnedStreamId};
use arrayvec::ArrayString;
use fjall::Slice;
use nexus::{Id, Version};
use nexus_store::PendingEnvelope;
use nexus_store::error::AppendError;
use nexus_store::store::{CheckpointStore, RawEventStore, Subscription};
use std::fmt::Write;
use std::path::Path;
use tokio::sync::Notify;

/// Format a `Display` value into a stack-allocated reason label,
/// silently truncating at 128 bytes on a char boundary.
fn reason_label(value: &impl std::fmt::Display) -> ArrayString<128> {
    let mut buf = ArrayString::<128>::new();
    let _ = write!(buf, "{value}");
    buf
}

/// Format a lossy UTF-8 representation into a stack-allocated label,
/// silently truncating at 64 bytes on a char boundary.
fn lossy_label(bytes: &[u8]) -> ArrayString<64> {
    let mut buf = ArrayString::<64>::new();
    let _ = write!(buf, "{}", String::from_utf8_lossy(bytes));
    buf
}

/// Fjall-backed event store.
///
/// Holds the transactional keyspace and partitions: `streams` maps
/// ID bytes to a version counter for optimistic concurrency, `events`
/// stores event rows keyed by `[u16 BE id_len][id_bytes][u64 BE version]`.
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
    pub(crate) checkpoints: fjall::TxPartitionHandle,
    pub(crate) notify: Notify,
}

impl FjallStore {
    /// Create a builder for opening a `FjallStore` at the given path.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
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
        let id_bytes = id.as_ref();
        let expected_raw = expected_version.map_or(0, Version::as_u64);

        // Version check BEFORE empty-batch early return. An empty append
        // with a stale expected_version signals a stale caller — report the
        // conflict even though no data would be written.
        let mut tx = self.db.write_tx();

        let current_version = if let Some(version_bytes) = tx
            .get(&self.streams, id_bytes)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
        {
            let current_version = decode_stream_version(&version_bytes).map_err(|_| {
                AppendError::Store(FjallError::CorruptMeta {
                    stream_id: id.to_label(),
                })
            })?;
            // Optimistic concurrency check.
            if current_version != expected_raw {
                return Err(AppendError::Conflict {
                    stream_id: id.to_label(),
                    expected: expected_version,
                    actual: Version::new(current_version),
                });
            }
            current_version
        } else {
            // New stream: expected_version must be None.
            if expected_version.is_some() {
                return Err(AppendError::Conflict {
                    stream_id: id.to_label(),
                    expected: expected_version,
                    actual: None,
                });
            }
            0
        };

        // Empty batch: version was checked (or new stream with None), no work to do.
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
                    stream_id: id.to_label(),
                    expected: Version::new(current_version),
                    actual: Some(env.version()),
                })?;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: id.to_label(),
                    expected: Version::new(expected_env_version),
                    actual: Some(env.version()),
                });
            }
        }

        // Write each envelope into the events partition.
        let mut value_buf = Vec::new();
        for env in envelopes {
            let key = encode_event_key(id_bytes, env.version().as_u64()).map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: id.to_label(),
                    version: env.version().as_u64(),
                    reason: reason_label(&e),
                })
            })?;
            encode_event_value(
                &mut value_buf,
                env.schema_version(),
                env.event_type(),
                env.payload(),
            )
            .map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: id.to_label(),
                    version: env.version().as_u64(),
                    reason: reason_label(&e),
                })
            })?;
            tx.insert(&self.events, &key, Slice::from(&*value_buf));
        }

        // Update stream version.
        let new_version = envelopes
            .last()
            .map_or(current_version, |last| last.version().as_u64());

        tx.insert(&self.streams, id_bytes, encode_stream_version(new_version));

        // Atomic cross-partition commit.
        tx.commit()
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?;

        self.notify.notify_waiters();

        Ok(())
    }

    async fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let id_bytes = id.as_ref();

        // Check if the stream exists. If not, return an empty stream.
        if self.streams.get(id_bytes)?.is_none() {
            return Ok(FjallStream {
                events: vec![],
                pos: 0,
                stream_id: id.to_label(),
                poisoned: false,
                #[cfg(debug_assertions)]
                prev_version: None,
            });
        }

        // Range scan from (id_bytes, from_version) to (id_bytes, u64::MAX).
        let start =
            encode_event_key(id_bytes, from.as_u64()).map_err(|e| FjallError::InvalidInput {
                stream_id: id.to_label(),
                version: from.as_u64(),
                reason: reason_label(&e),
            })?;
        let end = encode_event_key(id_bytes, u64::MAX).map_err(|e| FjallError::InvalidInput {
            stream_id: id.to_label(),
            version: u64::MAX,
            reason: reason_label(&e),
        })?;

        // Precondition: start key must sort <= end key (same ID bytes).
        debug_assert!(
            start <= end,
            "read_stream precondition: start key must sort <= end key"
        );

        let events: Vec<_> = self
            .events
            .inner()
            .range(start..=end)
            .map(|r| r.map(|kv| (kv.0, kv.1)))
            .collect::<Result<_, _>>()?;

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
            stream_id: id.to_label(),
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
    use crate::encoding::{decode_snapshot_value, encode_snapshot_value};
    use nexus_store::snapshot::{PendingSnapshot, PersistedSnapshot, SnapshotStore};

    impl SnapshotStore for FjallStore {
        type Error = FjallError;

        async fn load_snapshot(
            &self,
            id: &impl Id,
            expected_schema_version: std::num::NonZeroU32,
        ) -> Result<Option<PersistedSnapshot>, FjallError> {
            let id_bytes = id.as_ref();

            // Check if the stream exists.
            if self.streams.get(id_bytes)?.is_none() {
                return Ok(None);
            }

            // Snapshot key is the ID bytes directly.
            let Some(bytes) = self.snapshots.get(id_bytes)? else {
                return Ok(None);
            };

            let (schema_version_raw, version_raw, payload) = decode_snapshot_value(&bytes)
                .map_err(|_| FjallError::CorruptValue {
                    stream_id: id.to_label(),
                    version: None,
                })?;

            // Filter by schema version before constructing the snapshot
            // (avoids cloning payload bytes for stale snapshots).
            if schema_version_raw != expected_schema_version.get() {
                return Ok(None);
            }

            let version = Version::new(version_raw).ok_or_else(|| FjallError::CorruptValue {
                stream_id: id.to_label(),
                version: None,
            })?;
            let snap = PersistedSnapshot::new(version, expected_schema_version, payload.to_vec());
            Ok(Some(snap))
        }

        async fn save_snapshot(
            &self,
            id: &impl Id,
            snapshot: &PendingSnapshot,
        ) -> Result<(), FjallError> {
            let id_bytes = id.as_ref();

            // Check if stream exists — can't snapshot an aggregate with no events.
            let mut tx = self.db.write_tx();
            if tx
                .get(&self.streams, id_bytes)
                .map_err(FjallError::Io)?
                .is_none()
            {
                return Ok(());
            }

            let mut buf = Vec::new();
            encode_snapshot_value(
                &mut buf,
                snapshot.schema_version().get(),
                snapshot.version().as_u64(),
                snapshot.payload(),
            );
            tx.insert(&self.snapshots, id_bytes, fjall::Slice::from(&*buf));

            tx.commit().map_err(FjallError::Io)?;
            Ok(())
        }

        async fn delete_snapshot(&self, id: &impl Id) -> Result<(), FjallError> {
            let id_bytes = id.as_ref();
            let mut tx = self.db.write_tx();

            if tx
                .get(&self.streams, id_bytes)
                .map_err(FjallError::Io)?
                .is_none()
            {
                return Ok(());
            }

            tx.remove(&self.snapshots, id_bytes);
            tx.commit().map_err(FjallError::Io)?;
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Subscription implementation
// ═══════════════════════════════════════════════════════════════════════════

impl Subscription<()> for FjallStore {
    type Stream<'a> = FjallSubscriptionStream<'a>;
    type Error = FjallError;

    async fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> Result<FjallSubscriptionStream<'a>, FjallError> {
        let start = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(FjallError::VersionOverflow)?,
        };
        let owned_id = OwnedStreamId::from_id(id);
        let label = id.to_label();
        let inner = self.read_stream(&owned_id, start).await?;
        Ok(FjallSubscriptionStream::new(
            self, owned_id, label, inner, from,
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CheckpointStore implementation
// ═══════════════════════════════════════════════════════════════════════════

impl CheckpointStore for FjallStore {
    type Error = FjallError;

    fn load(
        &self,
        subscription_id: &impl Id,
    ) -> impl std::future::Future<Output = Result<Option<Version>, FjallError>> + Send + '_ {
        let key: Vec<u8> = subscription_id.as_ref().to_vec();
        async move {
            let Some(bytes) = self.checkpoints.get(&key)? else {
                return Ok(None);
            };
            // Value: u64 big-endian (8 bytes)
            let raw_bytes: [u8; 8] =
                bytes
                    .as_ref()
                    .try_into()
                    .map_err(|_| FjallError::CorruptValue {
                        stream_id: lossy_label(&key),
                        version: None,
                    })?;
            let raw = u64::from_be_bytes(raw_bytes);
            Version::new(raw).map_or_else(
                || {
                    Err(FjallError::CorruptValue {
                        stream_id: lossy_label(&key),
                        version: Some(raw),
                    })
                },
                |v| Ok(Some(v)),
            )
        }
    }

    fn save(
        &self,
        subscription_id: &impl Id,
        version: Version,
    ) -> impl std::future::Future<Output = Result<(), FjallError>> + Send + '_ {
        let key: Vec<u8> = subscription_id.as_ref().to_vec();
        async move {
            self.checkpoints
                .insert(&key, version.as_u64().to_be_bytes())?;
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

    impl AsRef<[u8]> for TestId {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }

    impl nexus::Id for TestId {
        const BYTE_LEN: usize = 0;
    }

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

        assert!(stream.next().await.unwrap().is_none());
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
        assert!(stream.next().await.unwrap().is_none());
    }
}
