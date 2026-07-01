use crate::builder::FjallStoreBuilder;
use crate::error::FjallError;
use crate::global_seq::GlobalSeq;
use crate::partition::{AllIndex, Partitions};
use crate::plan;
use crate::scan::{GlobalScan, ScanCursor, StreamScan};
use crate::subscription_id::OwnedStreamId;
use nexus::{ErrorId, Version};
use nexus_store::PendingEnvelope;
use nexus_store::StreamKey;
use nexus_store::error::AppendError;
use nexus_store::notify::{NotifyError, StreamNotifiers, WakeReg};
use nexus_store::store::RawEventStore;
use nexus_store::wake::WakeSource;
use std::path::Path;
use std::sync::Arc;

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
    /// The opened keyspaces and the on-disk layout — the one owner of every
    /// partition read/write (see [`Partitions`]).
    pub(crate) partitions: Partitions,
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

    /// Optimistic-concurrency check: the caller's `expected` version must match
    /// the stream's `current` version. `current == 0` denotes a new stream, for
    /// which `expected` must be `None`.
    fn check_optimistic(
        current: u64,
        expected: Option<Version>,
        id: &StreamKey,
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
}

/// Map a neutral [`plan::PlanError`] into the single-stream [`AppendError`]
/// domain. A version-sequence overflow becomes a `Store(VersionOverflow)`, NOT
/// a `Conflict` — overflow is not a retry-eligible concurrency conflict (rule
/// 3), and this matches the atomic-append path's handling.
fn append_plan_err(id: &StreamKey, e: &plan::PlanError) -> AppendError<FjallError> {
    match *e {
        plan::PlanError::Conflict { expected, actual } => AppendError::Conflict {
            stream_id: ErrorId::from_display(id),
            expected,
            actual,
        },
        plan::PlanError::VersionOverflow => AppendError::Store(FjallError::VersionOverflow),
        plan::PlanError::GlobalSeqOverflow => AppendError::Store(FjallError::GlobalSeqOverflow),
        plan::PlanError::InvalidInput { version, reason } => {
            AppendError::Store(FjallError::InvalidInput {
                stream_id: ErrorId::from_display(id),
                version,
                reason,
            })
        }
    }
}

impl RawEventStore for FjallStore {
    type Error = FjallError;
    type Stream = ScanCursor<StreamScan>;
    type AllPosition = GlobalSeq;
    type AllStream = ScanCursor<GlobalScan>;

    #[allow(
        clippy::significant_drop_tightening,
        reason = "tx must be held across concurrency check + inserts + commit"
    )]
    async fn append(
        &self,
        id: &StreamKey,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
    ) -> Result<(), AppendError<Self::Error>> {
        let id_bytes = id.as_ref();

        // Version check BEFORE empty-batch early return. An empty append
        // with a stale expected_version signals a stale caller — report the
        // conflict even though no data would be written.
        let mut tx = self.db.write_tx();

        let current_version = self
            .partitions
            .read_version(&tx, id)
            .map_err(AppendError::Store)?;
        Self::check_optimistic(current_version, expected_version, id)?;

        // Empty batch: version was checked (or new stream with None), no work to do.
        if envelopes.is_empty() {
            return Ok(());
        }

        // Read the current store-global sequence counter (monotonic, shared
        // across all streams). Absent key = no events appended yet.
        let current_global = self
            .partitions
            .read_global(&tx, id)
            .map_err(AppendError::Store)?;

        // Pure plan: validate strict-sequential versions and encode/stage every
        // event with a running GlobalSeq. The entire write body lives in
        // `plan::plan_run`, unit-tested with no fjall (mirrors postgres
        // `prepare_inserts`) — the same core the atomic-append path stages with.
        let planned = plan::plan_run(current_version, current_global, id, envelopes)
            .map_err(|e| append_plan_err(id, &e))?;

        // Stage each event into both indexes via the one dual-write site, then
        // advance both counters — all in the same transaction so they commit
        // together. The batch is non-empty (checked above), so `plan_run` staged
        // at least one event and advanced the global counter.
        for row in &planned.rows {
            self.partitions.stage_event(&mut tx, row);
        }
        self.partitions.set_global(&mut tx, planned.ending_global);
        self.partitions
            .set_version(&mut tx, id_bytes, planned.new_version);

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

    async fn read_stream(
        &self,
        id: &StreamKey,
        from: Version,
    ) -> Result<Self::Stream, Self::Error> {
        // A single bounded range scan; a nonexistent stream simply yields an
        // empty range, so no separate existence check is needed.
        ScanCursor::open(
            self.partitions.events(),
            StreamScan {
                id: OwnedStreamId::from_id(id),
                label: ErrorId::from_display(id),
            },
            from,
        )
    }

    async fn read_all(&self, from: Option<GlobalSeq>) -> Result<Self::AllStream, Self::Error> {
        // A store built with `AllIndex::Disabled` maintains no `$all` index, so
        // there is nothing to scan — surface it explicitly (produce-and-sync
        // configuration; append + per-stream reads are unaffected).
        if self.partitions.all_index() == AllIndex::Disabled {
            return Err(FjallError::AllIndexDisabled);
        }
        // `$all` resume is **exclusive**: open strictly after `from`. The
        // `ScanCursor` keyset bound is inclusive, so resume at `from.next()`
        // (None = from the first position). At the `GlobalSeq::MAX` ceiling
        // there is no successor — nothing is strictly after it — so the resume
        // anchor collapses to `None` and the cursor is empty, never a re-read of
        // the ceiling event. Mirrors `StreamCatchup::read_after` in nexus-store.
        from.map_or(Some(GlobalSeq::INITIAL), GlobalSeq::next)
            .map_or_else(
                || {
                    Ok(ScanCursor::open_empty(
                        self.partitions.events_global(),
                        GlobalScan,
                    ))
                },
                |after| ScanCursor::open(self.partitions.events_global(), GlobalScan, after),
            )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SnapshotStore<Vec<u8>, Version> implementation
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
mod snapshot_impl {
    use super::{ErrorId, FjallError, FjallStore, Version};
    use crate::snapshot::{decode_snapshot_value, encode_snapshot_value};
    use nexus::Id;
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
            let Some(bytes) = self.partitions.read_snapshot(id.as_ref())? else {
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
            self.partitions.write_snapshot(id.as_ref(), &buf)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AtomicAppend — commit N per-stream runs in ONE cross-partition transaction
// (export/import, issue #145 + #220). The primitive `Atomicity::WholeChunk`
// import needs and that per-stream `RawEventStore::append` cannot provide.
// ═══════════════════════════════════════════════════════════════════════════

/// Mirrors `InMemoryStore::atomic_append_many`'s two-phase discipline, but the
/// whole batch lands in a single fjall `write_tx` spanning every partition
/// (`streams`, `events`, `events_global`, `global`): either every run commits
/// or — on any conflict, overflow, or encode failure — the transaction is
/// dropped uncommitted and **nothing** lands across **any** partition (CLAUDE
/// rule 1). A second `write_tx` or a per-stream commit loop would be the bug
/// this is built to prevent.
#[cfg(feature = "import")]
mod atomic_append_impl {
    use super::{ErrorId, FjallError, FjallStore, StreamKey, Version};
    use crate::plan;
    use nexus_store::import::{AtomicAppend, AtomicAppendError, PlannedAppend};
    use std::collections::HashMap;

    type Tx<'a> = fjall::SingleWriterWriteTx<'a>;

    /// Map a neutral [`plan::PlanError`] into the [`AtomicAppendError`] domain.
    /// A per-run sequential violation surfaces as `Conflict { index }`, but it is
    /// unreachable in practice: `validate_atomic_writes` already proved every run
    /// sequential before any staging. Overflow / encode failures map to `Store`,
    /// never `Conflict` (rule 3).
    fn atomic_plan_err(
        index: usize,
        target: &StreamKey,
        e: &plan::PlanError,
    ) -> AtomicAppendError<FjallError> {
        match *e {
            plan::PlanError::Conflict { .. } => AtomicAppendError::Conflict {
                index,
                actual: None,
            },
            plan::PlanError::VersionOverflow => {
                AtomicAppendError::Store(FjallError::VersionOverflow)
            }
            plan::PlanError::GlobalSeqOverflow => {
                AtomicAppendError::Store(FjallError::GlobalSeqOverflow)
            }
            plan::PlanError::InvalidInput { version, reason } => {
                AtomicAppendError::Store(FjallError::InvalidInput {
                    stream_id: ErrorId::from_display(target),
                    version,
                    reason,
                })
            }
        }
    }

    impl FjallStore {
        /// Phase 1 — validate every write's head against a RUNNING projected
        /// head; NO mutation, NO partition writes. Tracking prior same-batch
        /// writes to a target makes a non-injective route (two writes → one
        /// stream) conflict here instead of concatenating into a corrupt,
        /// non-monotonic stream (`AtomicAppend` distinct-targets contract). The
        /// projected head is keyed by raw id bytes — ids need not be UTF-8.
        fn validate_atomic_writes(
            &self,
            tx: &Tx<'_>,
            writes: &[PlannedAppend],
        ) -> Result<(), AtomicAppendError<FjallError>> {
            let mut projected: HashMap<Vec<u8>, u64> = HashMap::with_capacity(writes.len());
            for (index, w) in writes.iter().enumerate() {
                let key = w.target.as_ref().to_vec();
                let actual = match projected.get(&key) {
                    Some(&head) => head,
                    None => self
                        .partitions
                        .read_version(tx, &w.target)
                        .map_err(AtomicAppendError::Store)?,
                };
                let expected = w.expected_version.map_or(0, Version::as_u64);
                let conflict = || AtomicAppendError::Conflict {
                    index,
                    actual: Version::new(actual),
                };
                if actual != expected {
                    return Err(conflict());
                }
                // Defensive (each crate validates at its own boundary): the run
                // must be strictly sequential from expected + 1. A running
                // checked_add counter avoids any index→u64 cast (rule 2).
                let mut want = expected.checked_add(1);
                for env in &w.events {
                    let Some(want_version) = want else {
                        return Err(AtomicAppendError::Store(FjallError::VersionOverflow));
                    };
                    if env.version().as_u64() != want_version {
                        return Err(conflict());
                    }
                    want = want_version.checked_add(1);
                }
                // Advance this target's projected head by the run just validated,
                // so a later same-target write in this batch conflicts above.
                let new_head = w
                    .events
                    .last()
                    .map_or(actual, |last| last.version().as_u64());
                projected.insert(key, new_head);
            }
            Ok(())
        }

        /// Phase 2 — stage + write every run into all partitions in the SAME
        /// transaction, stamping each event with the next `GlobalSeq` via one
        /// running counter shared across all streams (monotonic; gaps
        /// permitted). `writes` is non-empty (the caller returns early on
        /// empty), so `writes[0]` labels a corrupt global-counter error.
        fn write_atomic_runs(
            &self,
            tx: &mut Tx<'_>,
            writes: &[PlannedAppend],
        ) -> Result<(), AtomicAppendError<FjallError>> {
            let start_global = self
                .partitions
                .read_global(tx, &writes[0].target)
                .map_err(AtomicAppendError::Store)?;
            let mut global = start_global;
            for (index, w) in writes.iter().enumerate() {
                // `validate_atomic_writes` already matched this run's head to the
                // running projected head, so plan from `expected_version` (== the
                // head). `plan_run` re-derives the very same staged rows the
                // single-stream `append` does — ONE encode implementation shared
                // by both write paths, threading the running GlobalSeq across runs.
                let current_version = w.expected_version.map_or(0, Version::as_u64);
                let planned = plan::plan_run(current_version, global, &w.target, &w.events)
                    .map_err(|e| atomic_plan_err(index, &w.target, &e))?;
                for row in &planned.rows {
                    self.partitions.stage_event(tx, row);
                }
                // Advance the stream version counter to the run's last version.
                // An empty run stages nothing (defensive; the planner guarantees
                // a non-empty run in practice).
                if !planned.rows.is_empty() {
                    self.partitions
                        .set_version(tx, w.target.as_ref(), planned.new_version);
                }
                global = planned.ending_global;
            }
            // Advance the global counter once, in the same transaction — only
            // when at least one event was assigned (an all-empty batch leaves it
            // untouched, mirroring append's empty-batch path).
            if global != start_global {
                self.partitions.set_global(tx, global);
            }
            Ok(())
        }
    }

    impl AtomicAppend for FjallStore {
        #[allow(
            clippy::significant_drop_tightening,
            reason = "the single write_tx is held across validation + every insert + commit so the whole batch is one atomic transaction (rule 1)"
        )]
        async fn atomic_append_many(
            &self,
            writes: &[PlannedAppend],
        ) -> Result<(), AtomicAppendError<FjallError>> {
            // Nothing to commit — don't open an empty transaction.
            if writes.is_empty() {
                return Ok(());
            }

            // One write_tx spans validation, every partition insert, and the
            // commit. Any early Err drops `tx` uncommitted → nothing lands
            // across any partition (the whole point of this card).
            let mut tx = self.db.write_tx();
            self.validate_atomic_writes(&tx, writes)?;
            self.write_atomic_runs(&mut tx, writes)?;
            tx.commit()
                .map_err(|e| AtomicAppendError::Store(FjallError::Io(e)))?;

            // Wake AFTER the durable commit so a woken subscriber re-reads
            // already-visible data: one wake per touched stream + the $all path.
            for w in writes {
                if !w.events.is_empty() {
                    self.notifiers.wake(w.target.as_ref());
                }
            }
            self.notifiers.wake_all();
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// StreamLister — lazily enumerate stream ids for export (issue #145 + #220).
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "export")]
mod stream_lister_impl {
    use super::{FjallError, FjallStore};
    use bytes::Bytes;
    use core::pin::Pin;
    use core::task::{Context, Poll};
    use nexus_store::StreamKey;
    use nexus_store::export::StreamLister;

    /// A lazy cursor over the `streams` partition's keys — each key is one
    /// stream id. Wraps a single snapshot-pinned `fjall::Iter` (a lazy k-way LSM
    /// merge that pulls the next block only when the current drains), so it
    /// streams ids in bounded memory rather than materializing them all (unlike
    /// `InMemoryStore`'s eager `collect()`). Snapshot-consistent at open: the id
    /// set is a torn-free point-in-time view.
    pub struct StreamIdCursor {
        iter: fjall::Iter,
        /// Once an error is yielded the cursor is poisoned: subsequent polls
        /// return `None` rather than silently skipping ids (mirrors `ScanCursor`).
        poisoned: bool,
    }

    impl StreamIdCursor {
        fn poll_one(&mut self) -> Option<Result<StreamKey, FjallError>> {
            if self.poisoned {
                return None;
            }
            // `key()` loads the key only (no value): an id enumeration never
            // needs the stored version counter, and the `bytes_1` feature makes
            // the `Slice → Bytes` conversion zero-copy into the `StreamKey`.
            match self.iter.next()?.key() {
                Ok(key) => Some(Ok(StreamKey::from_bytes(Bytes::from(key)))),
                Err(e) => {
                    self.poisoned = true;
                    Some(Err(FjallError::Io(e)))
                }
            }
        }
    }

    // `fjall::Iter` is `Unpin`, so the cursor is `Unpin` and `get_mut()` is sound.
    impl futures::Stream for StreamIdCursor {
        type Item = Result<StreamKey, FjallError>;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.get_mut().poll_one())
        }
    }

    impl StreamLister for FjallStore {
        type StreamList = StreamIdCursor;

        async fn list_streams(&self) -> Result<Self::StreamList, FjallError> {
            Ok(StreamIdCursor {
                iter: self.partitions.stream_ids(),
                poisoned: false,
            })
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
pub mod read_test_helpers {
    use super::*;
    use nexus_store::envelope::pending_envelope;

    /// Build a [`StreamKey`] for the in-crate tests — the byte-level store API
    /// speaks `StreamKey` directly (issue #245).
    pub fn sk(s: &str) -> StreamKey {
        StreamKey::from_slice(s.as_bytes())
    }

    // Opens a default store. The cursor is a single lazy `fjall::Iter` that
    // reads the whole range, so there is no batch knob to configure.
    pub fn temp_store() -> (FjallStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        (store, dir)
    }

    pub async fn seed(store: &FjallStore, id: &StreamKey, count: u64) {
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
    use crate::wire_key::encode_global_key;
    use fjall::Slice;
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::wire;

    fn sk(s: &str) -> StreamKey {
        StreamKey::from_slice(s.as_bytes())
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

    /// Build a valid V2 wire-frame value (`schema_version` 1, no metadata) for
    /// white-box insertion into the `events_global` partition — used by the
    /// corruption / burned-position tests that need to plant rows directly.
    fn frame_value(event_type: &str, payload: &[u8]) -> Slice {
        let sv = nexus_store::value::SchemaVersion::from_u32(1).unwrap();
        let et = nexus_store::value::EventType::from_bytes(bytes::Bytes::copy_from_slice(
            event_type.as_bytes(),
        ))
        .unwrap();
        let pl = nexus_store::value::Payload::from_bytes(bytes::Bytes::copy_from_slice(payload))
            .unwrap();
        Slice::from(wire::encode_frame(sv, &et, &pl, None).unwrap().value)
    }

    // ---- append tests ----

    #[tokio::test]
    async fn append_to_new_stream() {
        let (store, _dir) = temp_store();
        let env = make_envelope(1, "Created", b"payload");

        let result = store.append(&sk("stream-1"), None, &[env]).await;
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

        let result = store.append(&sk("stream-1"), None, &envs).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_detects_version_conflict() {
        let (store, _dir) = temp_store();
        let env1 = make_envelope(1, "Created", b"p1");
        store.append(&sk("stream-1"), None, &[env1]).await.unwrap();

        // Try appending with wrong expected version (None instead of Some(1)).
        let env2 = make_envelope(1, "Duplicate", b"p2");
        let result = store.append(&sk("stream-1"), None, &[env2]).await;
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
            .append(&sk(&long), Version::new(1), &[env])
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

        let result = store.append(&sk("stream-1"), Version::new(5), &[env]).await;
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
        let id = sk("acct-1");
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"x".to_vec())
            .unwrap()
            .build();
        store.append(&id, None, &[env]).await.unwrap();

        // The index holds exactly one row, keyed by [global_seq=1][version=1].
        let key = crate::wire_key::encode_global_key(1, 1);
        let got = store.partitions.events_global().inner().get(key).unwrap();
        assert!(
            got.is_some(),
            "append must write an events_global index row"
        );
        assert_eq!(
            store.partitions.events_global().inner().iter().count(),
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
        store.append(&sk("stream-1"), None, &envs).await.unwrap();

        let mut stream = store
            .read_stream(&sk("stream-1"), Version::new(1).unwrap())
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
            .append(&sk("s"), None, &[make_envelope(1, "A", b"")])
            .await
            .unwrap();
        store
            .append(&sk("s"), Version::new(1), &[make_envelope(2, "B", b"")])
            .await
            .unwrap();

        let mut stream = store
            .read_stream(&sk("s"), Version::new(1).unwrap())
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
        use crate::store::read_test_helpers::{seed, sk as btid, temp_store};
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
        use crate::store::read_test_helpers::{seed, sk as btid, temp_store};
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
        use crate::store::read_test_helpers::{seed, sk as btid, temp_store};
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
        let id = sk("s");
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
        let a = sk("a");
        let b = sk("b");

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

        let mut all = store.read_all(None).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.next().await {
            let (pos, env) = item.unwrap();
            seen.push((pos.as_u64(), env.payload().to_vec()));
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
    async fn read_all_from_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let a = sk("a");
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

        // Exclusive: strictly after position 1 → [2, 3].
        let mut all = store
            .read_all(Some(GlobalSeq::new(1).unwrap()))
            .await
            .unwrap();
        let mut seqs = Vec::new();
        while let Some(item) = all.next().await {
            seqs.push(item.unwrap().0.as_u64());
        }
        assert_eq!(seqs, vec![2, 3]);
    }

    #[tokio::test]
    async fn read_all_survives_reopen() {
        // Lifecycle: append V2 frames across two interleaved streams, close,
        // reopen, recover EVERY event with the SAME $all positions — the
        // key-derived positions persist across reopen, not just the payloads.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let a = sk("a");
        let b = sk("b");
        {
            let store = FjallStore::builder(&path).open().unwrap();
            // a@1, a@2, b@1, a@3 → positions 1,2,3,4 interleaving the streams.
            store
                .append(
                    &a,
                    None,
                    &[make_envelope(1, "A1", b"a1"), make_envelope(2, "A2", b"a2")],
                )
                .await
                .unwrap();
            store
                .append(&b, None, &[make_envelope(1, "B1", b"b1")])
                .await
                .unwrap();
            store
                .append(&a, Version::new(2), &[make_envelope(3, "A3", b"a3")])
                .await
                .unwrap();
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let mut all = store.read_all(None).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.next().await {
            let (pos, env) = item.unwrap();
            seen.push((
                pos.as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        assert_eq!(
            seen,
            vec![
                (1, "A1".to_owned(), b"a1".to_vec()),
                (2, "A2".to_owned(), b"a2".to_vec()),
                (3, "B1".to_owned(), b"b1".to_vec()),
                (4, "A3".to_owned(), b"a3".to_vec()),
            ],
            "read_all after reopen must recover every event with identical positions",
        );

        // The persisted global counter is intact: a post-reopen append
        // continues the $all sequence at position 5, strictly after 4.
        store
            .append(&a, Version::new(3), &[make_envelope(4, "A4", b"a4")])
            .await
            .unwrap();
        let mut after = store
            .read_all(Some(GlobalSeq::new(4).unwrap()))
            .await
            .unwrap();
        let mut rest = Vec::new();
        while let Some(item) = after.next().await {
            rest.push(item.unwrap().0.as_u64());
        }
        assert_eq!(
            rest,
            vec![5],
            "post-reopen append continues the $all sequence at 5",
        );
    }

    #[tokio::test]
    async fn read_all_poisons_on_corrupt_events_global_row() {
        // Lifecycle: write → corrupt a persisted events_global row value →
        // reopen → read_all surfaces a typed FjallError (poison), never a
        // panic, never a silent skip.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let store = FjallStore::builder(&path).open().unwrap();
            store
                .append(&sk("a"), None, &[make_envelope(1, "E", b"ok")])
                .await
                .unwrap();
            // Overwrite the events_global row value (global_seq 1, version 1)
            // with too few bytes to be a valid frame, committed durably.
            let mut tx = store.db.write_tx();
            tx.insert(
                store.partitions.events_global(),
                encode_global_key(1, 1),
                Slice::from(&[0u8, 1, 2][..]),
            );
            tx.commit().unwrap();
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let mut all = store.read_all(None).await.unwrap();
        match all.next().await {
            Some(Err(FjallError::CorruptValue { version, .. })) => {
                assert_eq!(version, Some(1), "corruption surfaces the row's version");
            }
            other => panic!("expected CorruptValue, got {other:?}"),
        }
        assert!(
            all.next().await.is_none(),
            "poisoned cursor must terminate, not silently skip the corrupt row",
        );
    }

    #[tokio::test]
    async fn read_all_from_max_yields_empty() {
        // Defensive boundary: nothing is strictly after GlobalSeq::MAX, so the
        // exclusive resume opens an empty cursor (ScanCursor::open_empty) — no
        // panic, no re-read of the ceiling event.
        let (store, _dir) = temp_store();
        store
            .append(&sk("a"), None, &[make_envelope(1, "E", b"x")])
            .await
            .unwrap();
        let mut all = store
            .read_all(Some(GlobalSeq::new(u64::MAX).unwrap()))
            .await
            .unwrap();
        assert!(
            all.next().await.is_none(),
            "read_all(Some(MAX)) must yield nothing",
        );
    }

    #[tokio::test]
    async fn read_all_tolerates_burned_position_gap() {
        // Monotonic-but-NOT-gapless: simulate an aborted append that burned
        // position 2 by planting events_global rows at 1 and 3 (skipping 2).
        // The $all read is a range scan, so it tolerates the gap and never
        // assumes next == prev + 1.
        let (store, _dir) = temp_store();
        {
            let mut tx = store.db.write_tx();
            tx.insert(
                store.partitions.events_global(),
                encode_global_key(1, 1),
                frame_value("E", b"p1"),
            );
            tx.insert(
                store.partitions.events_global(),
                encode_global_key(3, 1),
                frame_value("E", b"p3"),
            );
            tx.commit().unwrap();
        }

        let mut all = store.read_all(None).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.next().await {
            let (pos, env) = item.unwrap();
            seen.push((pos.as_u64(), env.payload().to_vec()));
        }
        assert_eq!(
            seen,
            vec![(1, b"p1".to_vec()), (3, b"p3".to_vec())],
            "read_all must yield across a burned position, never assuming contiguity",
        );

        // Exclusive resume strictly after 1 skips the burned 2 and yields 3.
        let mut after = store
            .read_all(Some(GlobalSeq::new(1).unwrap()))
            .await
            .unwrap();
        let mut rest = Vec::new();
        while let Some(item) = after.next().await {
            rest.push(item.unwrap().0.as_u64());
        }
        assert_eq!(
            rest,
            vec![3],
            "resume after 1 tolerates the burned 2, yields 3",
        );
    }

    // ---- linearizability / concurrency tests ----

    #[allow(clippy::as_conversions, reason = "test code: v as u8 for payload byte")]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test code: v stays within 0..=20"
    )]
    #[tokio::test]
    async fn reader_sees_monotonic_gapfree_while_writer_appends() {
        use crate::store::read_test_helpers::{seed, sk as btid, temp_store};
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

    // --- AllIndex::Disabled (produce-and-sync IoT config, #270) ---

    #[tokio::test]
    async fn all_index_disabled_skips_events_global_but_keeps_per_stream() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db"))
            .all_index(AllIndex::Disabled)
            .open()
            .unwrap();
        let id = sk("device-1");
        for v in 1..=5u64 {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("Reading")
                .payload(b"p".to_vec())
                .unwrap()
                .build();
            store
                .append(&id, Version::new(v - 1), &[env])
                .await
                .unwrap();
        }

        // Per-stream reads are unaffected — the events partition is written.
        let mut cur = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = cur.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);

        // The $all index was NOT written (the whole point — reclaim the copy).
        assert_eq!(
            store.partitions.events_global().inner().iter().count(),
            0,
            "AllIndex::Disabled must not write the events_global index"
        );

        // read_all surfaces the disabled state explicitly, not an empty stream.
        // (matches! avoids requiring the AllStream Ok-type to impl Debug.)
        assert!(
            matches!(
                store.read_all(None).await,
                Err(FjallError::AllIndexDisabled)
            ),
            "expected AllIndexDisabled from an AllIndex::Disabled store"
        );
    }

    #[tokio::test]
    async fn all_index_default_is_denormalized_and_writes_events_global() {
        let (store, _dir) = temp_store();
        let id = sk("s");
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("E")
            .payload(b"p".to_vec())
            .unwrap()
            .build();
        store.append(&id, None, &[env]).await.unwrap();
        // Default (Denormalized) writes the $all index and read_all works.
        assert_eq!(store.partitions.events_global().inner().iter().count(), 1);
        assert!(store.read_all(None).await.is_ok());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AtomicAppend white-box tests (issue #220). These reach the `pub(crate)`
// partition handles directly, so they live in-crate rather than in `tests/`:
// the freeze-gating guarantee is "nothing lands across ANY partition", which
// can only be proven by inspecting every partition. Gated on `import` so they
// run under the default-feature `nix flake check` gate via the self
// dev-dependency.
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(all(test, feature = "import"))]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod atomic_append_tests {
    use super::*;
    use crate::store::read_test_helpers::{sk, temp_store};
    use crate::wire_key::decode_stream_version;
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::import::{AtomicAppend, AtomicAppendError, PlannedAppend};
    use std::sync::Arc as StdArc;
    use tokio::sync::Barrier;

    fn pending(version: u64, payload: &[u8]) -> PendingEnvelope {
        pending_envelope(Version::new(version).unwrap())
            .event_type("E")
            .payload(payload.to_vec())
            .unwrap()
            .build()
    }

    fn planned(target: &str, expected: Option<u64>, versions: &[u64]) -> PlannedAppend {
        PlannedAppend {
            target: sk(target),
            expected_version: expected.and_then(Version::new),
            events: versions.iter().map(|n| pending(*n, b"p")).collect(),
        }
    }

    async fn read_versions(store: &FjallStore, id: &StreamKey) -> Vec<u64> {
        let mut stream = store.read_stream(id, Version::INITIAL).await.unwrap();
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item.unwrap().version().as_u64());
        }
        out
    }

    // ── White-box partition probes ──────────────────────────────────────────

    fn row_count(ks: &fjall::SingleWriterTxKeyspace) -> usize {
        ks.inner().iter().count()
    }

    fn stream_counter(store: &FjallStore, id: &StreamKey) -> Option<u64> {
        store
            .partitions
            .streams()
            .inner()
            .get(id.as_ref())
            .unwrap()
            .map(|v| decode_stream_version(&v).unwrap())
    }

    fn global_counter(store: &FjallStore) -> Option<u64> {
        store
            .partitions
            .global()
            .inner()
            .get(b"global_seq")
            .unwrap()
            .map(|b| u64::from_le_bytes(<[u8; 8]>::try_from(b.as_ref()).unwrap()))
    }

    /// Snapshot every partition's observable state so a test can assert "nothing
    /// changed across ANY partition" after a rolled-back transaction.
    fn partition_snapshot(store: &FjallStore) -> (usize, usize, Option<u64>) {
        (
            row_count(store.partitions.events()),
            row_count(store.partitions.events_global()),
            global_counter(store),
        )
    }

    // ── Category 1: sequence / protocol ─────────────────────────────────────

    #[tokio::test]
    async fn atomic_append_many_commits_all_runs() {
        let (store, _dir) = temp_store();
        let writes = vec![planned("a", None, &[1, 2]), planned("b", None, &[1])];
        store.atomic_append_many(&writes).await.unwrap();

        assert_eq!(read_versions(&store, &sk("a")).await, vec![1, 2]);
        assert_eq!(read_versions(&store, &sk("b")).await, vec![1]);
        // Three events total → events + events_global each hold 3 rows, and the
        // shared global counter advanced to 3.
        assert_eq!(row_count(store.partitions.events()), 3);
        assert_eq!(row_count(store.partitions.events_global()), 3);
        assert_eq!(global_counter(&store), Some(3));
        assert_eq!(stream_counter(&store, &sk("a")), Some(2));
        assert_eq!(stream_counter(&store, &sk("b")), Some(1));
    }

    #[tokio::test]
    async fn atomic_append_then_normal_append_shares_counters() {
        // The two append paths share the version + global counters: a normal
        // append must continue exactly where an atomic batch left off.
        let (store, _dir) = temp_store();
        store
            .atomic_append_many(&[planned("a", None, &[1, 2])])
            .await
            .unwrap();
        store
            .append(&sk("a"), Version::new(2), &[pending(3, b"p")])
            .await
            .unwrap();

        assert_eq!(read_versions(&store, &sk("a")).await, vec![1, 2, 3]);
        // global_seq is contiguous across the two paths: 1,2 (atomic) then 3.
        assert_eq!(global_counter(&store), Some(3));
        let mut all = store.read_all(None).await.unwrap();
        let mut seqs = Vec::new();
        while let Some(item) = all.next().await {
            seqs.push(item.unwrap().0.as_u64());
        }
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn empty_writes_commit_nothing() {
        let (store, _dir) = temp_store();
        store.atomic_append_many(&[]).await.unwrap();
        assert_eq!(partition_snapshot(&store), (0, 0, None));
    }

    // ── Category 2: lifecycle (write → close → reopen) ──────────────────────

    #[tokio::test]
    async fn atomic_append_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let store = FjallStore::builder(&path).open().unwrap();
            store
                .atomic_append_many(&[planned("a", None, &[1, 2]), planned("b", None, &[1])])
                .await
                .unwrap();
        }
        let store = FjallStore::builder(&path).open().unwrap();
        assert_eq!(read_versions(&store, &sk("a")).await, vec![1, 2]);
        assert_eq!(read_versions(&store, &sk("b")).await, vec![1]);
        // A post-reopen normal append continues the global sequence (4th event
        // → global_seq 4), proving the counter persisted.
        store
            .append(&sk("a"), Version::new(2), &[pending(3, b"p")])
            .await
            .unwrap();
        assert_eq!(global_counter(&store), Some(4));
    }

    // ── Category 3: defensive boundary — the FREEZE GATE ────────────────────

    #[tokio::test]
    async fn conflict_rolls_back_every_partition() {
        // THE freeze-gating test (CLAUDE rule 1). Pre-seed "b" to v1 so the
        // second write (expecting fresh) conflicts. Write "a"'s clean run is
        // index 0; "b"'s conflict is index 1. A per-stream commit loop would
        // have committed "a" before failing "b"; the single write_tx must leave
        // EVERY partition byte-identical to the pre-import snapshot.
        let (store, _dir) = temp_store();
        store
            .append(&sk("b"), None, &[pending(1, b"seed")])
            .await
            .unwrap();
        let before = partition_snapshot(&store);

        let writes = vec![planned("a", None, &[1, 2]), planned("b", None, &[1])];
        let err = store.atomic_append_many(&writes).await.unwrap_err();
        match err {
            AtomicAppendError::Conflict { index, actual } => {
                assert_eq!(index, 1);
                assert_eq!(actual, Version::new(1));
            }
            // AtomicAppendError is #[non_exhaustive] (public error enum); the
            // wildcard covers Store and any future variant.
            other => panic!("expected Conflict, got {other}"),
        }

        // Nothing landed anywhere: "a" has no events + no version counter, the
        // event/global row counts and the global seq counter are unchanged.
        assert_eq!(read_versions(&store, &sk("a")).await, Vec::<u64>::new());
        assert_eq!(stream_counter(&store, &sk("a")), None);
        assert_eq!(read_versions(&store, &sk("b")).await, vec![1], "b intact");
        assert_eq!(stream_counter(&store, &sk("b")), Some(1));
        assert_eq!(
            partition_snapshot(&store),
            before,
            "every partition must be byte-identical to before the aborted import",
        );
    }

    #[tokio::test]
    async fn non_injective_route_conflicts_no_corruption() {
        // Two writes to the SAME target, both expecting a fresh stream. The
        // second conflicts against the running projected head (set by the
        // first) — never a concatenated [1,2,1,2] stream.
        let (store, _dir) = temp_store();
        let writes = vec![planned("t", None, &[1, 2]), planned("t", None, &[1, 2])];
        let err = store.atomic_append_many(&writes).await.unwrap_err();
        match err {
            AtomicAppendError::Conflict { index, actual } => {
                assert_eq!(index, 1);
                // The first write projected "t"'s head to 2.
                assert_eq!(actual, Version::new(2));
            }
            // AtomicAppendError is #[non_exhaustive] (public error enum).
            other => panic!("expected Conflict, got {other}"),
        }
        // All-or-nothing: "t" is empty, definitely not [1,2,1,2].
        assert_eq!(read_versions(&store, &sk("t")).await, Vec::<u64>::new());
        assert_eq!(partition_snapshot(&store), (0, 0, None));
    }

    #[tokio::test]
    async fn defensive_non_sequential_run_conflicts_and_rolls_back() {
        // The contract says the caller guarantees a contiguous run; fjall still
        // validates defensively. A run [1, 3] (internal gap) must be rejected
        // with nothing committed.
        let (store, _dir) = temp_store();
        let writes = vec![planned("a", None, &[1, 3])];
        let err = store.atomic_append_many(&writes).await.unwrap_err();
        assert!(matches!(err, AtomicAppendError::Conflict { index: 0, .. }));
        assert_eq!(partition_snapshot(&store), (0, 0, None));
    }

    // ── Category 4: linearizability / isolation ─────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_reader_sees_a_valid_prefix_during_atomic_append() {
        // A concurrent reader draining "a" while a multi-event atomic batch
        // commits must only ever observe a VALID gap-free prefix of [1,2,3] —
        // never a gap, reorder, duplicate, or phantom (e.g. "b"'s events). A
        // reader catching the stream mid-growth at [1] or [1,2] is fine: that is
        // a valid prefix, not a torn view. (The batch's *all-or-nothing*
        // guarantee is about the writer's committed/rolled-back state — proven
        // by `conflict_rolls_back_every_partition` and
        // `aborted_batch_never_exposes_its_writes_to_a_reader` — not about
        // mid-commit read isolation of a non-transactional scan, which fjall
        // does not promise and event sourcing does not need.)
        let (raw, _dir) = temp_store();
        let store = StdArc::new(raw);
        let barrier = StdArc::new(Barrier::new(2));

        let writer = StdArc::clone(&store);
        let wbarrier = StdArc::clone(&barrier);
        let handle = tokio::spawn(async move {
            wbarrier.wait().await;
            writer
                .atomic_append_many(&[planned("a", None, &[1, 2, 3]), planned("b", None, &[1])])
                .await
                .unwrap();
        });

        barrier.wait().await;
        // Poll the reader repeatedly to maximise the chance of catching the
        // stream mid-growth; every observation must be a contiguous 1..=len
        // prefix (no gap/reorder/dup) and never exceed the 3 appended events.
        for _ in 0..64u32 {
            let seen = read_versions(&store, &sk("a")).await;
            let valid_prefix: Vec<u64> = (1u64..).take(seen.len()).collect();
            assert_eq!(
                seen, valid_prefix,
                "concurrent read saw a torn (gapped/reordered) view, not a valid prefix",
            );
            assert!(
                seen.len() <= 3,
                "saw more than the 3 appended events: {seen:?}"
            );
        }
        handle.await.unwrap();
        assert_eq!(read_versions(&store, &sk("a")).await, vec![1, 2, 3]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn aborted_batch_never_exposes_its_writes_to_a_reader() {
        // The cross-partition rollback under a concurrent reader. "a" is seeded
        // to v1; a doomed batch tries to extend "a" to v3 while "b" (pre-seeded)
        // conflicts. A concurrent reader on "a" must NEVER see v2/v3, and "a"
        // must finish at exactly [1] — proving "a"'s would-be writes were rolled
        // back atomically rather than committed before "b" failed.
        let (raw, _dir) = temp_store();
        let store = StdArc::new(raw);
        store
            .append(&sk("a"), None, &[pending(1, b"seed")])
            .await
            .unwrap();
        store
            .append(&sk("b"), None, &[pending(1, b"seed")])
            .await
            .unwrap();

        let barrier = StdArc::new(Barrier::new(2));
        let writer = StdArc::clone(&store);
        let wbarrier = StdArc::clone(&barrier);
        let handle = tokio::spawn(async move {
            wbarrier.wait().await;
            let batch = vec![planned("a", Some(1), &[2, 3]), planned("b", None, &[1])];
            // Doomed: "b" is already at v1, so the batch must abort.
            writer.atomic_append_many(&batch).await.unwrap_err();
        });

        barrier.wait().await;
        for _ in 0..64u32 {
            let seen = read_versions(&store, &sk("a")).await;
            assert_eq!(
                seen,
                vec![1],
                "aborted batch leaked a write to the reader: {seen:?}"
            );
        }
        handle.await.unwrap();
        assert_eq!(read_versions(&store, &sk("a")).await, vec![1]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_read_all_never_sees_a_torn_atomic_batch() {
        // A one-shot $all reader racing a multi-stream atomic_append_many must
        // only ever observe a position-ordered view that is EITHER empty (batch
        // not yet committed) OR the WHOLE committed batch — never a torn subset,
        // never another stream's un-committed events. The batch's 4 events
        // ([a:1,2,3] + [b:1]) all land in ONE write_tx, so a snapshot-pinned
        // read_all cannot straddle the commit.
        let (raw, _dir) = temp_store();
        let store = StdArc::new(raw);
        let barrier = StdArc::new(Barrier::new(2));

        let writer = StdArc::clone(&store);
        let wbarrier = StdArc::clone(&barrier);
        let handle = tokio::spawn(async move {
            wbarrier.wait().await;
            writer
                .atomic_append_many(&[planned("a", None, &[1, 2, 3]), planned("b", None, &[1])])
                .await
                .unwrap();
        });

        barrier.wait().await;
        for _ in 0..64u32 {
            let mut all = store.read_all(None).await.unwrap();
            let mut positions = Vec::new();
            while let Some(item) = all.next().await {
                positions.push(item.unwrap().0.as_u64());
            }
            assert!(
                positions.is_empty() || positions == vec![1, 2, 3, 4],
                "read_all saw a torn view of an atomic batch: {positions:?}",
            );
        }
        handle.await.unwrap();

        // After the batch commits, read_all yields exactly the 4 events in order.
        let mut all = store.read_all(None).await.unwrap();
        let mut positions = Vec::new();
        while let Some(item) = all.next().await {
            positions.push(item.unwrap().0.as_u64());
        }
        assert_eq!(positions, vec![1, 2, 3, 4]);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// StreamLister tests (issue #220) — gated on `export`.
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(all(test, feature = "export"))]
#[allow(clippy::unwrap_used, reason = "test code")]
mod stream_lister_tests {
    use super::*;
    use crate::store::read_test_helpers::{seed, sk, temp_store};
    use futures::StreamExt;
    use nexus_store::export::StreamLister;
    use std::collections::HashSet;

    async fn list_ids(store: &FjallStore) -> HashSet<Vec<u8>> {
        let stream = store.list_streams().await.unwrap();
        stream
            .map(|r| r.unwrap().as_bytes().to_vec())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    #[tokio::test]
    async fn lists_exactly_the_appended_ids() {
        let (store, _dir) = temp_store();
        seed(&store, &sk("alpha"), 1).await;
        seed(&store, &sk("beta"), 2).await;
        seed(&store, &sk("gamma"), 3).await;

        let ids = list_ids(&store).await;
        let expected: HashSet<Vec<u8>> = [b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()]
            .into_iter()
            .collect();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn empty_store_lists_nothing() {
        let (store, _dir) = temp_store();
        assert!(list_ids(&store).await.is_empty());
    }

    #[tokio::test]
    async fn lists_each_stream_once_regardless_of_event_count() {
        // A 50-event stream is one id, not 50 — the lister scans the `streams`
        // partition (one row per stream), never the `events` partition.
        let (store, _dir) = temp_store();
        seed(&store, &sk("busy"), 50).await;
        let stream = store.list_streams().await.unwrap();
        let all: Vec<Vec<u8>> = stream
            .map(|r| r.unwrap().as_bytes().to_vec())
            .collect::<Vec<_>>()
            .await;
        assert_eq!(all, vec![b"busy".to_vec()]);
    }

    #[tokio::test]
    async fn list_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let store = FjallStore::builder(&path).open().unwrap();
            seed(&store, &sk("a"), 1).await;
            seed(&store, &sk("b"), 1).await;
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let ids = list_ids(&store).await;
        assert_eq!(
            ids,
            [b"a".to_vec(), b"b".to_vec()]
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
        );
    }

    #[tokio::test]
    async fn handles_long_stream_ids() {
        // fjall keys are the raw id bytes; lsm-tree rejects an *empty* key, so
        // an empty stream id is unsupported at the `append` boundary (a
        // pre-existing adapter limitation, not the lister's). A long id is the
        // boundary the lister must round-trip faithfully.
        let (store, _dir) = temp_store();
        let long = "x".repeat(300);
        seed(&store, &sk(&long), 1).await;
        seed(&store, &sk("short"), 1).await;
        let ids = list_ids(&store).await;
        assert!(ids.contains(long.as_bytes()));
        assert!(ids.contains(b"short".as_slice()));
    }
}
