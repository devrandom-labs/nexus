//! fjall-private parameterization of a bounded keyset scan: the only parts
//! that differ between the per-stream (Version-keyed) and $all (GlobalSeq-keyed)
//! reads — the keyset bound bytes and how a stored row decodes into a
//! [`PersistedEnvelope`]. NOT exported; no other adapter shares fjall's on-disk
//! key layout, so this stays inside `nexus-fjall`.

use bytes::Bytes;
use fjall::Slice;
use nexus::{ErrorId, Version};
use nexus_store::{GlobalSeq, PersistedEnvelope};

use crate::error::{FjallError, reason_label};
use crate::subscription_id::OwnedStreamId;
use crate::wire_key::{
    DecodedEvent, decode_event_key, decode_event_value, decode_global_key, encode_event_key,
    encode_global_key,
};

/// The differing parts of a bounded keyset scan, factored so one cursor can
/// drive both the per-stream and `$all` reads.
pub trait ScanStrategy: Send {
    /// The position the scan opens from ([`Version`] for a stream, [`GlobalSeq`] for `$all`).
    type Position: Copy + Send;
    /// Keyset lower bound (inclusive) for opening at `from`.
    fn lower_key(&self, from: Self::Position) -> Result<Vec<u8>, FjallError>;
    /// Keyset upper bound (inclusive) — the end of this strategy's key range.
    fn upper_key(&self) -> Result<Vec<u8>, FjallError>;
    /// Decode one stored row into an envelope, mapping malformed shapes to `FjallError`.
    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError>;
}

/// Per-stream scan: keyed by `[id_len][id_bytes][version]`, opens from a [`Version`].
pub struct StreamScan {
    pub id: OwnedStreamId,
    pub label: ErrorId,
}

/// `$all` scan: keyed by `[global_seq][version]`, opens from a [`GlobalSeq`].
pub struct GlobalScan;

/// Shared decode tail: validate the raw `(version, global_seq)` and build the
/// envelope, mapping the two terminal failures identically for both strategies.
///
/// `stream_id` is the diagnostic label stamped into every error: the per-stream
/// label for [`StreamScan`], an empty [`ErrorId`] for [`GlobalScan`] (a
/// global key carries no stream id). `raw_version` is the version decoded from
/// the key; it appears verbatim in the error fields.
fn build_envelope(
    bytes_value: Bytes,
    decoded: DecodedEvent,
    raw_version: u64,
    stream_id: ErrorId,
) -> Result<PersistedEnvelope, FjallError> {
    let version = Version::new(raw_version).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(raw_version),
    })?;
    let global_seq = GlobalSeq::new(decoded.global_seq).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(raw_version),
    })?;

    PersistedEnvelope::try_new(
        version,
        global_seq,
        bytes_value,
        decoded.schema_version,
        decoded.event_type_range,
        decoded.payload_range,
        decoded.metadata_range,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id,
        version: raw_version,
        source,
    })
}

impl ScanStrategy for StreamScan {
    type Position = Version;

    fn lower_key(&self, from: Self::Position) -> Result<Vec<u8>, FjallError> {
        encode_event_key(self.id.as_ref(), from.as_u64()).map_err(|e| FjallError::InvalidInput {
            stream_id: self.label,
            version: from.as_u64(),
            reason: reason_label(&e),
        })
    }

    fn upper_key(&self) -> Result<Vec<u8>, FjallError> {
        encode_event_key(self.id.as_ref(), u64::MAX).map_err(|e| FjallError::InvalidInput {
            stream_id: self.label,
            version: u64::MAX,
            reason: reason_label(&e),
        })
    }

    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError> {
        let (_id_bytes, version) = decode_event_key(key).map_err(|_| FjallError::CorruptValue {
            stream_id: self.label,
            version: None,
        })?;

        let bytes_value: Bytes = value.into();
        let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
            stream_id: self.label,
            version: Some(version),
        })?;

        build_envelope(bytes_value, decoded, version, self.label)
    }
}

impl ScanStrategy for GlobalScan {
    type Position = GlobalSeq;

    fn lower_key(&self, from: Self::Position) -> Result<Vec<u8>, FjallError> {
        Ok(encode_global_key(from.as_u64(), 0).to_vec())
    }

    fn upper_key(&self) -> Result<Vec<u8>, FjallError> {
        Ok(encode_global_key(u64::MAX, u64::MAX).to_vec())
    }

    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError> {
        let (key_global_seq, version_raw) =
            decode_global_key(key).map_err(|_| FjallError::CorruptValue {
                stream_id: ErrorId::default(),
                version: None,
            })?;

        let bytes_value: Bytes = value.into();
        let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
            stream_id: ErrorId::default(),
            version: Some(version_raw),
        })?;

        // The global_seq in the key and in the frame value must match
        // (CLAUDE.md rule 4 — redundant data must be validated).
        if decoded.global_seq != key_global_seq {
            return Err(FjallError::CorruptValue {
                stream_id: ErrorId::default(),
                version: Some(version_raw),
            });
        }

        build_envelope(bytes_value, decoded, version_raw, ErrorId::default())
    }
}

/// A bounded read cursor over a single lazy `fjall::Iter`.
///
/// `fjall::Keyspace::range` returns a lazy k-way-merge cursor over LSM blocks
/// (it pulls the next block from disk only when the current one drains), so a
/// single `Iter` already bounds memory — no batching/refill needed. Holding it
/// pins one consistent snapshot for the read's duration (repeatable-read).
pub struct ScanCursor<S: ScanStrategy> {
    iter: fjall::Iter,
    strategy: S,
    /// Once an error is yielded the cursor is poisoned: subsequent polls return
    /// `None` rather than silently skipping corrupt rows.
    poisoned: bool,
}

impl<S: ScanStrategy> ScanCursor<S> {
    /// Open a bounded scan from `from` (inclusive). Fallible only because the
    /// keyset bound keys may fail to encode (e.g. an over-long id).
    ///
    /// The snapshot is taken **at `open` time**, not at first poll: the returned
    /// cursor reads a consistent point-in-time view as of `open`, so events
    /// appended after `open()` but before/while polling are **not** observed.
    /// Long-lived use therefore pins the GC watermark — a bounded read completes
    /// promptly, but a never-ending subscription must open a **fresh**
    /// [`ScanCursor`] per refill rather than hold one for its whole life.
    pub fn open(
        keyspace: &fjall::SingleWriterTxKeyspace,
        strategy: S,
        from: S::Position,
    ) -> Result<Self, FjallError> {
        let lower = strategy.lower_key(from)?;
        let upper = strategy.upper_key()?;
        let iter = keyspace.inner().range(lower..=upper);
        Ok(Self {
            iter,
            strategy,
            poisoned: false,
        })
    }

    fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        let guard = self.iter.next()?;
        let (key, value) = match guard.into_inner() {
            Ok(kv) => kv,
            Err(e) => {
                self.poisoned = true;
                return Some(Err(FjallError::Io(e)));
            }
        };
        match self.strategy.decode(&key, value) {
            Ok(env) => Some(Ok(env)),
            Err(e) => {
                self.poisoned = true;
                Some(Err(e))
            }
        }
    }
}

// `get_mut()` in `poll_next` requires `Self: Unpin`; `fjall::Iter` is already
// `Unpin`, so `S` is the only field that isn't `Unpin` by default — hence the
// `S: Unpin` bound (also relied on by the generic live loop in `nexus-store`).
impl<S: ScanStrategy + Unpin> futures::Stream for ScanCursor<S> {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.get_mut().poll_one())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use crate::store::FjallStore;
    use crate::store::read_test_helpers::{Tid, temp_store, tid};
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::store::RawEventStore;
    use nexus_store::value::{EventType, Payload, SchemaVersion};
    use nexus_store::wire;

    /// Build a wire-frame event-value row via the real production encoder
    /// (`wire::encode_frame` + the `nexus_store::value` newtypes), for the
    /// row-decode tests below. `schema_version` is always 1 and there is no
    /// metadata — the cases this test mod exercises.
    fn test_row_value(global_seq: u64, event_type: &str, payload: &[u8]) -> Vec<u8> {
        let sv = SchemaVersion::from_u32(1).unwrap();
        let et = EventType::from_bytes(Bytes::copy_from_slice(event_type.as_bytes())).unwrap();
        let pl = Payload::from_bytes(Bytes::copy_from_slice(payload)).unwrap();
        wire::encode_frame(global_seq, sv, &et, &pl, None)
            .unwrap()
            .value
            .to_vec()
    }

    async fn append_versions(
        store: &FjallStore,
        id: &Tid,
        versions: std::ops::RangeInclusive<u64>,
    ) {
        for v in versions {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(format!("v{v}").into_bytes())
                .unwrap()
                .build();
            store.append(id, Version::new(v - 1), &[env]).await.unwrap();
        }
    }

    #[tokio::test]
    async fn scan_cursor_yields_rows_in_order() {
        let (store, _dir) = temp_store();
        let id = tid("s");
        append_versions(&store, &id, 1..=3).await;

        let cursor = ScanCursor::open(
            &store.events,
            StreamScan {
                id: OwnedStreamId::from_id(&id),
                label: ErrorId::from_display(&id),
            },
            Version::INITIAL,
        )
        .unwrap();

        let versions: Vec<u64> = cursor
            .map(|item| item.unwrap().version().as_u64())
            .collect()
            .await;
        assert_eq!(versions, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn scan_cursor_opens_from_midpoint() {
        let (store, _dir) = temp_store();
        let id = tid("s");
        append_versions(&store, &id, 1..=5).await;

        let cursor = ScanCursor::open(
            &store.events,
            StreamScan {
                id: OwnedStreamId::from_id(&id),
                label: ErrorId::from_display(&id),
            },
            Version::new(3).unwrap(),
        )
        .unwrap();

        let versions: Vec<u64> = cursor
            .map(|item| item.unwrap().version().as_u64())
            .collect()
            .await;
        assert_eq!(versions, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn scan_cursor_global_yields_ascending_global_seq() {
        let (store, _dir) = temp_store();
        let a = tid("a");
        let b = tid("b");

        // Interleave appends across two streams so global_seq order differs
        // from per-stream version order.
        append_versions(&store, &a, 1..=1).await; // global_seq 1
        append_versions(&store, &b, 1..=1).await; // global_seq 2
        append_versions(&store, &a, 2..=2).await; // global_seq 3
        append_versions(&store, &b, 2..=2).await; // global_seq 4

        let cursor =
            ScanCursor::open(&store.events_global, GlobalScan, GlobalSeq::INITIAL).unwrap();

        let seqs: Vec<u64> = cursor
            .map(|item| item.unwrap().global_seq().as_u64())
            .collect()
            .await;
        assert_eq!(seqs, vec![1, 2, 3, 4]);
    }

    fn stream_scan(id_bytes: &[u8], label: &str) -> StreamScan {
        StreamScan {
            id: OwnedStreamId::from_id(&label_id(id_bytes)),
            label: ErrorId::from_display(&label),
        }
    }

    /// A minimal [`nexus::Id`] over borrowed bytes, used only to feed
    /// [`OwnedStreamId::from_id`] in tests.
    fn label_id(bytes: &[u8]) -> TestId {
        TestId(bytes.to_vec())
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct TestId(Vec<u8>);
    impl std::fmt::Display for TestId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", String::from_utf8_lossy(&self.0))
        }
    }
    impl AsRef<[u8]> for TestId {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    impl nexus::Id for TestId {
        const BYTE_LEN: usize = 0;
    }

    fn row(id: &[u8], version: u64, global_seq: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_event_key(id, version).unwrap();
        let val = test_row_value(global_seq, et, payload);
        (Slice::from(key), Slice::from(val))
    }

    fn global_row(global_seq: u64, version: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_global_key(global_seq, version);
        let val = test_row_value(global_seq, et, payload);
        (Slice::from(&key[..]), Slice::from(val))
    }

    #[test]
    fn stream_decode_yields_envelope() {
        let (k, v) = row(b"user-1", 7, 42, "Created", b"data");
        let scan = stream_scan(b"user-1", "user-1");
        let env = scan.decode(&k, v).unwrap();
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn stream_decode_rejects_truncated_value() {
        let k = Slice::from(encode_event_key(b"corrupt", 1).unwrap());
        let v = Slice::from(&[0u8, 1, 2][..]);
        let scan = stream_scan(b"corrupt", "corrupt");
        match scan.decode(&k, v).unwrap_err() {
            FjallError::CorruptValue { stream_id, version } => {
                assert_eq!(stream_id.as_str(), "corrupt");
                assert_eq!(version, Some(1));
            }
            other => panic!("expected CorruptValue, got {other:?}"),
        }
    }

    #[test]
    fn stream_decode_rejects_non_utf8_event_type() {
        // `decode_event_value` no longer UTF-8-validates `event_type`; the
        // read path's `PersistedEnvelope::try_new` does, surfacing it as
        // `FjallError::EnvelopeCorrupt`. Build a valid frame, then overwrite
        // the `event_type` bytes in place (same length) with invalid UTF-8.
        let (k, v) = row(b"user-1", 7, 42, "ABC", b"data");
        let mut raw = v.to_vec();
        let et_start = crate::wire_key::EVENT_VALUE_HEADER_SIZE;
        // 0xFF is never a valid UTF-8 byte; keep the 3-byte length intact.
        raw[et_start] = 0xFF;
        raw[et_start + 1] = 0xFE;
        raw[et_start + 2] = 0xFF;
        let corrupt = Slice::from(raw);

        let scan = stream_scan(b"user-1", "user-1");
        match scan.decode(&k, corrupt).unwrap_err() {
            FjallError::EnvelopeCorrupt {
                stream_id, version, ..
            } => {
                assert_eq!(stream_id.as_str(), "user-1");
                assert_eq!(version, 7);
            }
            other => panic!("expected EnvelopeCorrupt, got {other:?}"),
        }
    }

    #[test]
    fn global_decode_yields_envelope() {
        let (k, v) = global_row(42, 7, "Created", b"data");
        let env = GlobalScan.decode(&k, v).unwrap();
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn global_decode_rejects_global_seq_mismatch() {
        // Key says global_seq 99, value frame says 42 → corruption.
        let key = encode_global_key(99, 7);
        let val = test_row_value(42, "E", b"x");
        let err = GlobalScan
            .decode(&Slice::from(&key[..]), Slice::from(val))
            .unwrap_err();
        assert!(matches!(err, FjallError::CorruptValue { .. }));
    }
}
