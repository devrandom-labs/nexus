use crate::builder::FjallStoreBuilder;
use crate::encoding::{decode_stream_version, encode_event_key, encode_stream_version};
use crate::error::{FjallError, reason_label};
use crate::stream::FjallStream;
use crate::subscription_stream::{FjallSubscriptionStream, OwnedStreamId};
use fjall::{Readable, Slice};
use nexus::{Id, Version};
use nexus_store::PendingEnvelope;
use nexus_store::batch::BatchSize;
use nexus_store::error::AppendError;
use nexus_store::notify::StreamNotifiers;
use nexus_store::store::RawEventStore;
use nexus_store::subscription::{RawSubscription, sealed};
use nexus_store::wire;
use std::path::Path;
use std::sync::Arc;

/// The single key under which the store-global sequence counter is kept
/// in the `global` partition.
const GLOBAL_SEQ_KEY: &[u8] = b"global_seq";

/// Fjall-backed event store.
///
/// Holds the transactional keyspace and partitions: `streams` maps
/// ID bytes to a version counter for optimistic concurrency, `events`
/// stores event rows keyed by `[u16 BE id_len][id_bytes][u64 BE version]`.
///
/// The `global` partition holds the store-wide global sequence counter.
/// When the `snapshot` feature is enabled, an additional `snapshots`
/// partition is available for storing aggregate snapshots.
///
/// Use [`FjallStore::builder`] to configure and open a store.
pub struct FjallStore {
    pub(crate) db: fjall::SingleWriterTxDatabase,
    pub(crate) streams: fjall::SingleWriterTxKeyspace,
    pub(crate) events: fjall::SingleWriterTxKeyspace,
    #[cfg(feature = "snapshot")]
    pub(crate) snapshots: fjall::SingleWriterTxKeyspace,
    pub(crate) global: fjall::SingleWriterTxKeyspace,
    /// Per-stream wake registry. After a durable commit to stream `X`,
    /// `append` calls `wake(X)`, rousing only the subscribers parked on
    /// `X` rather than every subscriber in the store.
    pub(crate) notifiers: Arc<StreamNotifiers>,
    /// Maximum event rows materialized per read batch / refill.
    pub(crate) batch_size: BatchSize,
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
    type Stream = FjallStream;

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
        envelopes: &[PendingEnvelope],
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

        // Read the current store-global sequence counter (monotonic, shared
        // across all streams). Absent key = no events appended yet.
        let current_global = match tx
            .get(&self.global, GLOBAL_SEQ_KEY)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
        {
            Some(bytes) => {
                let raw: [u8; 8] = bytes.as_ref().try_into().map_err(|_| {
                    AppendError::Store(FjallError::CorruptMeta {
                        stream_id: id.to_label(),
                    })
                })?;
                u64::from_le_bytes(raw)
            }
            None => 0,
        };

        // Write each envelope into the events partition, stamping each with
        // the next GlobalSeq. The sequence is monotonic; gaps are permitted.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let global_seq = current_global
                .checked_add(1)
                .and_then(|v| v.checked_add(i_u64))
                .ok_or(AppendError::Store(FjallError::GlobalSeqOverflow))?;

            let key = encode_event_key(id_bytes, env.version().as_u64()).map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: id.to_label(),
                    version: env.version().as_u64(),
                    reason: reason_label(&e),
                })
            })?;
            let frame = wire::encode_frame(
                global_seq,
                env.schema_version_value(),
                &env.event_type_value(),
                &env.payload_value(),
                env.metadata_value().as_ref(),
            )
            .map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: id.to_label(),
                    version: env.version().as_u64(),
                    reason: reason_label(&e),
                })
            })?;
            tx.insert(&self.events, &key, Slice::from(frame.value));
        }

        // Advance the global counter by the batch size, in the same
        // transaction as the event writes — counter and events commit together.
        let batch_len = u64::try_from(envelopes.len()).unwrap_or(u64::MAX);
        let new_global = current_global
            .checked_add(batch_len)
            .ok_or(AppendError::Store(FjallError::GlobalSeqOverflow))?;
        tx.insert(&self.global, GLOBAL_SEQ_KEY, new_global.to_le_bytes());

        // Update stream version.
        let new_version = envelopes
            .last()
            .map_or(current_version, |last| last.version().as_u64());

        tx.insert(&self.streams, id_bytes, encode_stream_version(new_version));

        // Atomic cross-partition commit.
        tx.commit()
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?;

        // Wake only the subscribers parked on this stream, AFTER the commit
        // is durable so a woken subscriber re-reads already-visible data.
        self.notifiers.wake(id_bytes);

        Ok(())
    }

    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        // A single bounded range scan; a nonexistent stream simply yields an
        // empty first batch (done), so no separate existence check is needed.
        FjallStream::new(
            self.events.clone(),
            OwnedStreamId::from_id(id),
            from.as_u64(),
            self.batch_size.get(),
            id.to_label(),
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SnapshotStore<Vec<u8>, Version> implementation
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
mod snapshot_impl {
    use super::{FjallError, FjallStore, Id, Version};
    use crate::encoding::{decode_snapshot_value, encode_snapshot_value};
    use nexus_store::state::SnapshotStore;
    use std::num::NonZeroU32;

    impl SnapshotStore<Vec<u8>, Version> for FjallStore {
        type Error = FjallError;

        async fn hydrate(
            &self,
            id: &impl Id,
            schema_version: NonZeroU32,
        ) -> Result<Option<(Version, Vec<u8>)>, FjallError> {
            // Snapshot key is the ID bytes directly.
            let Some(bytes) = self.snapshots.get(id.as_ref())? else {
                return Ok(None);
            };

            let (schema_version_raw, version_raw, payload) = decode_snapshot_value(&bytes)
                .map_err(|_| FjallError::CorruptValue {
                    stream_id: id.to_label(),
                    version: None,
                })?;

            // Schema mismatch → treat as absent (avoids cloning a stale payload).
            if schema_version_raw != schema_version.get() {
                return Ok(None);
            }

            let version = Version::new(version_raw).ok_or_else(|| FjallError::CorruptValue {
                stream_id: id.to_label(),
                version: None,
            })?;

            Ok(Some((version, payload.to_vec())))
        }

        async fn commit(
            &self,
            id: &impl Id,
            schema_version: NonZeroU32,
            position: Version,
            state: &Vec<u8>,
        ) -> Result<(), FjallError> {
            let mut buf = Vec::new();
            encode_snapshot_value(&mut buf, schema_version.get(), position.as_u64(), state);
            self.snapshots
                .insert(id.as_ref(), fjall::Slice::from(&*buf))?;
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// RawSubscription impl
// ═══════════════════════════════════════════════════════════════════════════

impl sealed::Sealed for FjallStore {}

impl RawSubscription for FjallStore {
    type Stream = FjallSubscriptionStream;
    type Error = FjallError;

    async fn subscribe(
        arc: &Arc<Self>,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<FjallSubscriptionStream, FjallError> {
        let start = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(FjallError::VersionOverflow)?,
        };
        // Register the per-stream wake guard before the catch-up read, so the
        // map entry is live for any concurrent append's `wake`. The cursor
        // holds it for its whole life; dropping the cursor reaps the entry.
        let guard = arc.notifiers.subscribe(id.as_ref())?;
        let owned_id = OwnedStreamId::from_id(id);
        let label = id.to_label();
        let inner = arc.read_stream(&owned_id, start).await?;
        Ok(FjallSubscriptionStream::new(
            Arc::clone(arc),
            owned_id,
            label,
            inner,
            from,
            guard,
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
pub(crate) mod batch_test_helpers {
    use super::*;
    use nexus_store::batch::BatchSize;
    use nexus_store::envelope::pending_envelope;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    pub struct Tid(pub String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    pub fn tid(s: &str) -> Tid {
        Tid(s.to_owned())
    }

    pub fn store_with_batch(n: usize) -> (FjallStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db"))
            .batch_size(BatchSize::new(n).unwrap())
            .open()
            .unwrap();
        (store, dir)
    }

    pub async fn seed(store: &FjallStore, id: &Tid, count: u64) {
        for v in 1..=count {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(b"payload".to_vec())
                .unwrap()
                .build();
            store.append(id, Version::new(v - 1), &[env]).await.unwrap();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;

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

    fn make_envelope(version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope {
        pending_envelope(Version::new(version).expect("test version must be > 0"))
            .event_type(event_type)
            .payload(payload.to_vec())
            .expect("valid payload")
            .build()
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

    // ---- bounded-read (batch pagination) tests ----

    #[tokio::test]
    async fn read_yields_all_across_refills() {
        use crate::store::batch_test_helpers::{seed, store_with_batch, tid as btid};
        let (store, _dir) = store_with_batch(4);
        let id = btid("s");
        seed(&store, &id, 14).await;
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=14).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn read_terminates_at_exact_batch_boundary() {
        use crate::store::batch_test_helpers::{seed, store_with_batch, tid as btid};
        let (store, _dir) = store_with_batch(4);
        let id = btid("s");
        seed(&store, &id, 8).await; // exactly 2 full batches
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut count = 0u64;
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            item.unwrap();
            count += 1;
        }
        assert_eq!(count, 8);
    }

    #[tokio::test]
    async fn read_from_midpoint_resumes_across_refills() {
        use crate::store::batch_test_helpers::{seed, store_with_batch, tid as btid};
        let (store, _dir) = store_with_batch(3);
        let id = btid("s");
        seed(&store, &id, 10).await;
        let mut stream = store
            .read_stream(&id, Version::new(6).unwrap())
            .await
            .unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, vec![6, 7, 8, 9, 10]);
    }

    #[tokio::test]
    async fn read_reopened_store_recovers_all_across_refills() {
        use nexus_store::batch::BatchSize;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let id = tid("s");
        {
            let store = FjallStore::builder(&path)
                .batch_size(BatchSize::new(4).unwrap())
                .open()
                .unwrap();
            for v in 1..=10u64 {
                let env = make_envelope(v, "E", b"payload");
                store
                    .append(&id, Version::new(v - 1), &[env])
                    .await
                    .unwrap();
            }
        }
        let store = FjallStore::builder(&path)
            .batch_size(BatchSize::new(4).unwrap())
            .open()
            .unwrap();
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=10).collect::<Vec<_>>());
    }

    // ---- linearizability / concurrency tests ----

    #[allow(clippy::as_conversions, reason = "test code: v as u8 for payload byte")]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test code: v stays within 0..=20"
    )]
    #[tokio::test]
    async fn reader_sees_monotonic_gapfree_while_writer_appends() {
        use crate::store::batch_test_helpers::{seed, store_with_batch, tid as btid};
        use std::sync::Arc as StdArc;
        use tokio::sync::Barrier;

        let (raw_store, _dir) = store_with_batch(4);
        let store = StdArc::new(raw_store);
        let id = btid("s");
        seed(&store, &id, 4).await;

        // Force the writer and reader to start together so the reader's
        // pagination genuinely races concurrent appends.
        let barrier = StdArc::new(Barrier::new(2));

        let writer = StdArc::clone(&store);
        let wid = id.clone();
        let wbarrier = StdArc::clone(&barrier);
        let handle = tokio::spawn(async move {
            wbarrier.wait().await;
            for v in 5..=20u64 {
                let env = nexus_store::envelope::pending_envelope(Version::new(v).unwrap())
                    .event_type("E")
                    .payload(vec![v as u8])
                    .unwrap()
                    .build();
                writer
                    .append(&wid, Version::new(v - 1), &[env])
                    .await
                    .unwrap();
            }
        });

        // Concurrent read: assert a strictly monotonic, gap-free prefix
        // (linearizability — never observe a gap, reorder, dup, or phantom).
        barrier.wait().await;
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut prev = 0u64;
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            let v = item.unwrap().version().as_u64();
            assert_eq!(
                v,
                prev + 1,
                "concurrent read saw gap/reorder: {prev} -> {v}"
            );
            prev = v;
        }
        handle.await.unwrap();

        // Deterministic completeness check: after all writes commit, a fresh
        // paginating read must yield every event 1..=20 in order.
        let mut full = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut full).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=20).collect::<Vec<_>>());
    }
}
