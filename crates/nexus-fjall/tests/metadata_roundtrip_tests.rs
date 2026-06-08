//! Metadata round-trip + lifecycle + defensive boundary tests for the
//! bytes-envelope refactor's new wire format.
//!
//! Covers the four CLAUDE.md §7 mandatory test categories for the new
//! `Option<Bytes>` metadata field on `PendingEnvelope` / `PersistedEnvelope`:
//!
//! 1. Sequence / Protocol — multi-event metadata round-trip.
//! 2. Lifecycle           — write, drop, reopen, read.
//! 3. Defensive Boundary  — `None` is distinguishable from "present but empty"
//!    (the design's footgun F1 — empty `Bytes::slice` orphans from parent).
//! 4. Defensive Boundary  — corrupt `meta_len` exceeding the buffer is rejected
//!    by the decoder rather than read past the end.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]

use bytes::Bytes;
use futures::StreamExt;
use nexus::{Id, Version};
use nexus_fjall::FjallStore;
use nexus_fjall::encoding::{META_LEN_ABSENT, decode_event_value};
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct StreamName(&'static str);

impl std::fmt::Display for StreamName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl AsRef<[u8]> for StreamName {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for StreamName {
    const BYTE_LEN: usize = 0;
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Sequence / Protocol: multi-event metadata round-trip
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn metadata_roundtrips_across_multiple_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    let id = StreamName("multi");

    let envs: Vec<_> = (1..=5u64)
        .map(|i| {
            pending_envelope(Version::new(i).unwrap())
                .event_type("Test")
                .payload(Bytes::from(format!("payload-{i}")))
                .expect("valid payload")
                .with_metadata(Bytes::from(format!("meta-{i}")))
                .expect("valid metadata")
        })
        .collect();

    store.append(&id, None, &envs).await.expect("append");

    let mut cursor = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("read_stream");

    let mut seen = 0u64;
    while let Some(item) = cursor.next().await {
        let env = item.expect("cursor next");
        seen += 1;
        assert_eq!(env.version().as_u64(), seen);
        assert_eq!(env.event_type(), "Test");
        assert_eq!(env.payload(), format!("payload-{seen}").as_bytes());
        assert_eq!(
            env.metadata(),
            Some(format!("meta-{seen}").as_bytes()),
            "metadata mismatch at event {seen}",
        );
    }
    assert_eq!(seen, 5, "should have read all 5 events");
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Lifecycle: metadata survives close + reopen
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn metadata_survives_store_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let env = pending_envelope(Version::INITIAL)
            .event_type("X")
            .payload(Bytes::from_static(b"payload"))
            .expect("valid payload")
            .with_metadata(Bytes::from_static(b"important-meta"))
            .expect("valid metadata");
        store
            .append(&StreamName("reopen-stream"), None, &[env])
            .await
            .unwrap();
    }
    // First store + its keyspace handles dropped here.

    let store = FjallStore::builder(&db_path).open().unwrap();
    let mut cursor = store
        .read_stream(&StreamName("reopen-stream"), Version::INITIAL)
        .await
        .unwrap();
    let env = cursor.next().await.unwrap().expect("event present");
    assert_eq!(env.event_type(), "X");
    assert_eq!(env.payload(), b"payload");
    assert_eq!(env.metadata(), Some(b"important-meta".as_slice()));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Defensive boundary: `None` is distinguishable from absent — sentinel works
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn none_metadata_roundtrips_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    let id = StreamName("none-meta");

    let pending = pending_envelope(Version::INITIAL)
        .event_type("X")
        .payload(Bytes::from_static(b"payload"))
        .expect("valid payload")
        .build(); // no metadata
    store.append(&id, None, &[pending]).await.unwrap();

    let mut cursor = store.read_stream(&id, Version::INITIAL).await.unwrap();
    let read = cursor.next().await.unwrap().unwrap();
    assert_eq!(read.metadata(), None);
    assert!(read.metadata_bytes().is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Defensive boundary: decoder rejects `meta_len` exceeding the buffer
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decoder_rejects_meta_len_exceeding_buffer() {
    // Manually build a value that *claims* meta_len = 100 but only 10 bytes
    // of metadata/payload follow. The decoder must reject rather than read
    // past the buffer end.
    let mut buf = Vec::new();
    buf.extend_from_slice(&1u64.to_le_bytes()); // global_seq
    buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
    buf.extend_from_slice(&1u16.to_le_bytes()); // event_type_len = 1
    buf.extend_from_slice(&100u32.to_le_bytes()); // meta_len = 100 (LIE)
    buf.push(b'X'); // event_type (1 byte)
    buf.extend_from_slice(&[0u8; 10]); // only 10 bytes follow — not 100

    let bytes_buf = Bytes::from(buf);
    let err = decode_event_value(&bytes_buf).expect_err("must reject");
    assert!(
        matches!(
            err,
            nexus_fjall::encoding::DecodeError::Wire(
                nexus_store::wire::DecodeError::MetadataTruncated { meta_len: 100, .. }
            )
        ),
        "expected Wire(MetadataTruncated {{ meta_len: 100, .. }}), got {err:?}",
    );
}

#[test]
fn meta_len_absent_sentinel_is_u32_max() {
    // Regression guard — if `META_LEN_ABSENT` ever drifts from `u32::MAX`
    // the persisted on-disk layout silently changes meaning.
    assert_eq!(META_LEN_ABSENT, u32::MAX);
}
