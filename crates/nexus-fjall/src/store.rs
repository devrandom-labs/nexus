use crate::builder::FjallStoreBuilder;
use crate::error::{FjallError, reason_label};
use crate::scan::{GlobalScan, ScanCursor, StreamScan};
use crate::subscription_id::OwnedStreamId;
use crate::wire_key::{
    decode_stream_version, encode_event_key, encode_global_key, encode_stream_version,
};
use fjall::{Readable, Slice};
use nexus::{ErrorId, Id, Version};
use nexus_store::PendingEnvelope;
use nexus_store::error::AppendError;
use nexus_store::notify::{NotifyError, StreamNotifiers, WakeReg};
use nexus_store::store::{GlobalSeq, RawEventStore};
use nexus_store::wake::WakeSource;
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
    /// `$all` index: every event's wire frame keyed by
    /// `[u64 BE global_seq][u64 BE version]`. Same scan-optimized config as
    /// `events`; written in the same transaction as the primary row.
    pub(crate) events_global: fjall::SingleWriterTxKeyspace,
    #[cfg(feature = "snapshot")]
    pub(crate) snapshots: fjall::SingleWriterTxKeyspace,
    pub(crate) global: fjall::SingleWriterTxKeyspace,
    /// Per-stream wake registry. After a durable commit to stream `X`,
    /// `append` calls `wake(X)`, rousing only the subscribers parked on
    /// `X` rather than every subscriber in the store.
    pub(crate) notifiers: Arc<StreamNotifiers>,
}

impl FjallStore {
    /// Create a builder for opening a `FjallStore` at the given path.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
    }

    /// Point-read the current version counter for `id` from the `streams`
    /// partition within `tx`. Returns `0` for a stream that does not yet exist.
    fn read_current_version(
        &self,
        tx: &fjall::SingleWriterWriteTx<'_>,
        id: &impl Id,
    ) -> Result<u64, AppendError<FjallError>> {
        tx.get(&self.streams, id.as_ref())
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
            .map_or(Ok(0), |version_bytes| {
                decode_stream_version(&version_bytes).map_err(|_| {
                    AppendError::Store(FjallError::CorruptMeta {
                        stream_id: ErrorId::from_display(id),
                    })
                })
            })
    }

    /// Optimistic-concurrency check: the caller's `expected` version must match
    /// the stream's `current` version. `current == 0` denotes a new stream, for
    /// which `expected` must be `None`.
    fn check_optimistic(
        current: u64,
        expected: Option<Version>,
        id: &impl Id,
    ) -> Result<(), AppendError<FjallError>> {
        if current == 0 {
            // New stream: expected_version must be None.
            return if expected.is_some() {
                Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(id),
                    expected,
                    actual: None,
                })
            } else {
                Ok(())
            };
        }
        if current != expected.map_or(0, Version::as_u64) {
            return Err(AppendError::Conflict {
                stream_id: ErrorId::from_display(id),
                expected,
                actual: Version::new(current),
            });
        }
        Ok(())
    }

    /// Point-read the store-global sequence counter (monotonic, shared across
    /// all streams) from the `global` partition within `tx`. Absent key = `0`.
    fn read_current_global(
        &self,
        tx: &fjall::SingleWriterWriteTx<'_>,
        id: &impl Id,
    ) -> Result<u64, AppendError<FjallError>> {
        tx.get(&self.global, GLOBAL_SEQ_KEY)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
            .map_or(Ok(0), |bytes| {
                let raw: [u8; 8] = bytes.as_ref().try_into().map_err(|_| {
                    AppendError::Store(FjallError::CorruptMeta {
                        stream_id: ErrorId::from_display(id),
                    })
                })?;
                Ok(u64::from_le_bytes(raw))
            })
    }
}

impl RawEventStore for FjallStore {
    type Error = FjallError;
    type Stream = ScanCursor<StreamScan>;
    type AllStream = ScanCursor<GlobalScan>;

    #[allow(
        clippy::significant_drop_tightening,
        reason = "tx must be held across concurrency check + inserts + commit"
    )]
    async fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
    ) -> Result<(), AppendError<Self::Error>> {
        let id_bytes = id.as_ref();

        // Version check BEFORE empty-batch early return. An empty append
        // with a stale expected_version signals a stale caller — report the
        // conflict even though no data would be written.
        let mut tx = self.db.write_tx();

        let current_version = self.read_current_version(&tx, id)?;
        Self::check_optimistic(current_version, expected_version, id)?;

        // Empty batch: version was checked (or new stream with None), no work to do.
        if envelopes.is_empty() {
            return Ok(());
        }

        // Validate envelope versions are sequential from current_version + 1.
        // A running counter advanced by checked_add(1) avoids any usize -> u64
        // cast and guards against overflow near u64::MAX.
        let mut expected_version_seq =
            current_version
                .checked_add(1)
                .ok_or_else(|| AppendError::Conflict {
                    stream_id: ErrorId::from_display(id),
                    expected: Version::new(current_version),
                    actual: Some(envelopes[0].version()),
                })?;
        for env in envelopes {
            if env.version().as_u64() != expected_version_seq {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(id),
                    expected: Version::new(expected_version_seq),
                    actual: Some(env.version()),
                });
            }
            expected_version_seq =
                expected_version_seq
                    .checked_add(1)
                    .ok_or_else(|| AppendError::Conflict {
                        stream_id: ErrorId::from_display(id),
                        expected: Version::new(current_version),
                        actual: Some(env.version()),
                    })?;
        }

        // Read the current store-global sequence counter (monotonic, shared
        // across all streams). Absent key = no events appended yet.
        let current_global = self.read_current_global(&tx, id)?;

        // Write each envelope into the events partition, stamping each with
        // the next GlobalSeq via a running counter (monotonic; gaps permitted).
        let mut global_seq = current_global;
        for env in envelopes {
            global_seq = global_seq
                .checked_add(1)
                .ok_or(AppendError::Store(FjallError::GlobalSeqOverflow))?;

            let key = encode_event_key(id_bytes, env.version().as_u64()).map_err(|e| {
                AppendError::Store(FjallError::InvalidInput {
                    stream_id: ErrorId::from_display(id),
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
                    stream_id: ErrorId::from_display(id),
                    version: env.version().as_u64(),
                    reason: reason_label(&e),
                })
            })?;
            let slice = Slice::from(frame.value);
            tx.insert(&self.events, &key, slice.clone());
            let global_key = encode_global_key(global_seq, env.version().as_u64());
            tx.insert(&self.events_global, global_key, slice);
        }

        // Advance the global counter to the last assigned GlobalSeq, in the
        // same transaction as the event writes — counter and events commit
        // together. The write loop ran at least once (non-empty batch checked
        // above), so `global_seq` holds the last assigned value.
        tx.insert(&self.global, GLOBAL_SEQ_KEY, global_seq.to_le_bytes());

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
        // Wake every $all subscriber parked on the store-wide notifier.
        self.notifiers.wake_all();

        Ok(())
    }

    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        // A single bounded range scan; a nonexistent stream simply yields an
        // empty range, so no separate existence check is needed.
        ScanCursor::open(
            &self.events,
            StreamScan {
                id: OwnedStreamId::from_id(id),
                label: ErrorId::from_display(id),
            },
            from,
        )
    }

    async fn read_all(&self, from: GlobalSeq) -> Result<Self::AllStream, Self::Error> {
        ScanCursor::open(&self.events_global, GlobalScan, from)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SnapshotStore<Vec<u8>, Version> implementation
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
mod snapshot_impl {
    use super::{ErrorId, FjallError, FjallStore, Id, Version};
    use crate::snapshot::{decode_snapshot_value, encode_snapshot_value};
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
                    stream_id: ErrorId::from_display(id),
                    version: None,
                })?;

            // Schema mismatch → treat as absent (avoids cloning a stale payload).
            if schema_version_raw != schema_version.get() {
                return Ok(None);
            }

            let version = Version::new(version_raw).ok_or_else(|| FjallError::CorruptValue {
                stream_id: ErrorId::from_display(id),
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
// WakeSource impl — adapter-pluggable live wake for the generic subscription
// loop assembled in `nexus_store::subscription::Subscription`.
// ═══════════════════════════════════════════════════════════════════════════

/// Delegates wake-routing to the store's [`StreamNotifiers`]. `append` already
/// wakes the registry (per-stream + `$all`) after a durable commit, so a
/// registration armed before a concurrent append is roused once that append's
/// events are visible.
impl WakeSource for FjallStore {
    type Registration = WakeReg;
    type Error = NotifyError;

    fn register(&self, stream: Option<&[u8]>) -> Result<Self::Registration, Self::Error> {
        self.notifiers.register(stream)
    }

    fn wake(&self, stream: &[u8]) {
        self.notifiers.wake(stream);
        self.notifiers.wake_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
pub(crate) mod read_test_helpers {
    use super::*;
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

    // Opens a default store. The cursor is a single lazy `fjall::Iter` that
    // reads the whole range, so there is no batch knob to configure.
    pub fn temp_store() -> (FjallStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
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
            // AppendError is #[non_exhaustive] (#209): the wildcard covers
            // Store and any future variant — only Conflict is expected.
            other => panic!("expected Conflict, got: {other}"),
        }
    }

    #[tokio::test]
    async fn conflict_truncates_overlong_stream_id_with_ellipsis() {
        let (store, _dir) = temp_store();
        // A >64-byte id forces the ErrorId<64> stream-id label to truncate;
        // truncation must be visually signalled with a trailing ellipsis
        // (CLAUDE.md "truncation must be visually signalled").
        let long = "x".repeat(200);
        let env = make_envelope(1, "E", b"p");
        // New stream + Some(expected) → immediate optimistic-concurrency conflict.
        let err = store
            .append(&tid(&long), Version::new(1), &[env])
            .await
            .unwrap_err();
        match err {
            AppendError::Conflict { stream_id, .. } => {
                assert!(stream_id.as_str().len() <= 64);
                assert!(
                    stream_id.as_str().ends_with('…'),
                    "overlong stream id must be truncated with an ellipsis, got {stream_id:?}"
                );
            }
            // AppendError is #[non_exhaustive] (#209): the wildcard covers
            // Store and any future variant — only Conflict is expected.
            other => panic!("expected Conflict, got: {other}"),
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
            // AppendError is #[non_exhaustive] (#209): the wildcard covers
            // Store and any future variant — only Conflict is expected.
            other => panic!("expected Conflict, got: {other}"),
        }
    }

    #[tokio::test]
    async fn append_writes_events_global_index() {
        let (store, _dir) = temp_store();
        let id = tid("acct-1");
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"x".to_vec())
            .unwrap()
            .build();
        store.append(&id, None, &[env]).await.unwrap();

        // The index holds exactly one row, keyed by [global_seq=1][version=1].
        let key = crate::wire_key::encode_global_key(1, 1);
        let got = store.events_global.inner().get(key).unwrap();
        assert!(
            got.is_some(),
            "append must write an events_global index row"
        );
        assert_eq!(
            store.events_global.inner().iter().count(),
            1,
            "events_global must contain exactly one row after one append"
        );
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

    // ---- bounded-read tests (single lazy Iter, no pagination) ----

    #[tokio::test]
    async fn read_yields_all_rows_in_order() {
        use crate::store::read_test_helpers::{seed, temp_store, tid as btid};
        let (store, _dir) = temp_store();
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
    async fn read_terminates_at_end_of_stream() {
        use crate::store::read_test_helpers::{seed, temp_store, tid as btid};
        let (store, _dir) = temp_store();
        let id = btid("s");
        seed(&store, &id, 8).await;
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut count = 0u64;
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            item.unwrap();
            count += 1;
        }
        assert_eq!(count, 8);
    }

    #[tokio::test]
    async fn read_from_midpoint() {
        use crate::store::read_test_helpers::{seed, temp_store, tid as btid};
        let (store, _dir) = temp_store();
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
    async fn read_reopened_store_recovers_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let id = tid("s");
        {
            let store = FjallStore::builder(&path).open().unwrap();
            for v in 1..=10u64 {
                let env = make_envelope(v, "E", b"payload");
                store
                    .append(&id, Version::new(v - 1), &[env])
                    .await
                    .unwrap();
            }
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=10).collect::<Vec<_>>());
    }

    // ---- read_all tests ----

    #[tokio::test]
    async fn read_all_yields_global_order_across_streams() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let a = TestId("a".to_owned());
        let b = TestId("b".to_owned());

        let mk = |v: u64, p: &[u8]| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(p.to_vec())
                .unwrap()
                .build()
        };
        store.append(&a, None, &[mk(1, b"a1")]).await.unwrap();
        store.append(&b, None, &[mk(1, b"b1")]).await.unwrap();
        store
            .append(&a, Some(Version::new(1).unwrap()), &[mk(2, b"a2")])
            .await
            .unwrap();

        let mut all = store.read_all(GlobalSeq::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.next().await {
            let env = item.unwrap();
            seen.push((env.global_seq().as_u64(), env.payload().to_vec()));
        }
        assert_eq!(
            seen,
            vec![
                (1, b"a1".to_vec()),
                (2, b"b1".to_vec()),
                (3, b"a2".to_vec()),
            ],
        );
    }

    #[tokio::test]
    async fn read_all_from_is_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let a = TestId("a".to_owned());
        let mk = |v: u64| {
            #[allow(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "test code: v stays within 1..=3"
            )]
            pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(vec![v as u8])
                .unwrap()
                .build()
        };
        store.append(&a, None, &[mk(1)]).await.unwrap();
        store
            .append(&a, Some(Version::new(1).unwrap()), &[mk(2)])
            .await
            .unwrap();
        store
            .append(&a, Some(Version::new(2).unwrap()), &[mk(3)])
            .await
            .unwrap();

        let mut all = store.read_all(GlobalSeq::new(2).unwrap()).await.unwrap();
        let mut seqs = Vec::new();
        while let Some(env) = all.next().await {
            seqs.push(env.unwrap().global_seq().as_u64());
        }
        assert_eq!(seqs, vec![2, 3]);
    }

    #[tokio::test]
    async fn read_all_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let a = TestId("a".to_owned());
        {
            let store = FjallStore::builder(&path).open().unwrap();
            let env = pending_envelope(Version::new(1).unwrap())
                .event_type("E")
                .payload(b"x".to_vec())
                .unwrap()
                .build();
            store.append(&a, None, &[env]).await.unwrap();
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let mut all = store.read_all(GlobalSeq::INITIAL).await.unwrap();
        let env = all.next().await.unwrap().unwrap();
        assert_eq!(env.global_seq().as_u64(), 1);
        assert_eq!(env.payload(), b"x");
    }

    // ---- linearizability / concurrency tests ----

    #[allow(clippy::as_conversions, reason = "test code: v as u8 for payload byte")]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test code: v stays within 0..=20"
    )]
    #[tokio::test]
    async fn reader_sees_monotonic_gapfree_while_writer_appends() {
        use crate::store::read_test_helpers::{seed, temp_store, tid as btid};
        use std::sync::Arc as StdArc;
        use tokio::sync::Barrier;

        let (raw_store, _dir) = temp_store();
        let store = StdArc::new(raw_store);
        let id = btid("s");
        seed(&store, &id, 4).await;

        // Force the writer and reader to start together so the reader's
        // lazy scan genuinely races concurrent appends.
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
        // lazy scan must yield every event 1..=20 in order.
        let mut full = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut full).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=20).collect::<Vec<_>>());
    }
}
