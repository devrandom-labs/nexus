//! End-to-end export → CBOR-box → file → import coverage on the **persistent**
//! fjall adapter (issue #220). The nexus-store contract (`export.rs`,
//! `import.rs`, `cbor.rs`) is exercised everywhere against `InMemoryStore`;
//! this proves the same pipeline on disk, where the cross-partition
//! transactional guarantees actually have teeth (CLAUDE rule 1).
//!
//! White-box per-partition rollback assertions live in-crate (they need the
//! `pub(crate)` partition handles); this file covers the public, on-disk,
//! box-mediated path: real `list_streams` + `export_stream`, a real CBOR chunk
//! written to and read back from a temp file, and a real `import` into a fresh
//! store — plus the corruption-on-disk distinctions a restore must surface.
//!
//! Gated on `export`+`import`; the crate's self dev-dependency turns those on
//! under the default-feature `nix flake check` gate, and the `nexus-store`
//! `cbor` dev-dependency supplies the box codec.

#![cfg(all(feature = "export", feature = "import"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_arguments,
    reason = "test harness — relaxed lints for test code"
)]

use std::collections::BTreeSet;

use futures::StreamExt;
use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_store::StreamKey;
use nexus_store::cbor::{ChunkError, ChunkWriter, decode_chunk};
use nexus_store::envelope::{PersistedEnvelope, pending_envelope};
use nexus_store::export::{EventExporter, StreamLister};
use nexus_store::import::{AbortReason, Atomicity, EventImporter, ImportError, StreamOutcome};
use nexus_store::store::{GlobalSeq, RawEventStore};

fn sk(s: &str) -> StreamKey {
    StreamKey::from_slice(s.as_bytes())
}

/// Route a section's origin id bytes straight back to the same target key.
fn identity_route(origin: &[u8]) -> StreamKey {
    StreamKey::from_slice(origin)
}

fn open_store(path: &std::path::Path) -> FjallStore {
    FjallStore::builder(path).open().expect("open store")
}

/// Append one event with the given fields, advancing the stream by one.
async fn append_one(
    store: &FjallStore,
    id: &StreamKey,
    version: u64,
    event_type: &'static str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) {
    let builder = pending_envelope(Version::new(version).expect("nonzero"))
        .event_type(event_type)
        .payload(payload.to_vec())
        .expect("valid payload");
    let env = match metadata {
        Some(m) => builder.with_metadata(m.to_vec()).expect("valid metadata"),
        None => builder.build(),
    };
    let expected = Version::new(version - 1);
    store.append(id, expected, &[env]).await.expect("append");
}

/// Drain a stream into owned envelopes (whole stream from v1).
async fn collect_stream(store: &FjallStore, id: &StreamKey) -> Vec<PersistedEnvelope> {
    store
        .export_stream(id, Version::INITIAL)
        .await
        .expect("export opens")
        .map(|r| r.expect("no read error"))
        .collect()
        .await
}

/// Build a CBOR backup chunk for the given streams by walking the store's own
/// `list_streams` + `export_stream` — the exact production export path.
async fn build_chunk(store: &FjallStore) -> Vec<u8> {
    // Discover ids lazily, then sort for a deterministic chunk layout (list
    // order is unspecified).
    let mut ids: Vec<Vec<u8>> = store
        .list_streams()
        .await
        .expect("list opens")
        .map(|r| r.expect("no list error").as_bytes().to_vec())
        .collect::<Vec<_>>()
        .await;
    ids.sort();

    let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
    for id_bytes in ids {
        let id = identity_route(&id_bytes);
        let events = store
            .read_stream(&id, Version::INITIAL)
            .await
            .expect("read opens");
        w.section(&id_bytes)
            .expect("section")
            .try_extend(events)
            .await
            .expect("extend");
    }
    w.into_sink()
}

/// Assert two stores hold byte-identical streams **modulo `global_seq`** (export
/// preserves version/schema/type/payload/metadata; import re-stamps a fresh
/// `global_seq` on re-append).
async fn assert_streams_equal_modulo_global_seq(
    src: &FjallStore,
    dst: &FjallStore,
    id: &StreamKey,
) {
    let a = collect_stream(src, id).await;
    let b = collect_stream(dst, id).await;
    assert_eq!(a.len(), b.len(), "stream {id} length differs");
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.version(), y.version(), "version differs");
        assert_eq!(x.schema_version(), y.schema_version(), "schema differs");
        assert_eq!(x.event_type(), y.event_type(), "event_type differs");
        assert_eq!(x.payload(), y.payload(), "payload differs");
        assert_eq!(x.metadata(), y.metadata(), "metadata differs");
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Category 2 (lifecycle) + sequence: full on-disk round-trip
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn persistent_round_trip_through_a_file_preserves_streams() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let chunk_file = src_dir.path().join("backup.nxch");

    // 1. Seed a source store with two streams of varied shape.
    let src = open_store(&src_dir.path().join("src-db"));
    append_one(&src, &sk("task-1"), 1, "Created", Some(b"hlc=1"), b"alpha").await;
    append_one(&src, &sk("task-1"), 2, "Updated", None, b"beta").await;
    append_one(&src, &sk("task-1"), 3, "Closed", Some(b"hlc=3"), b"gamma").await;
    append_one(&src, &sk("task-2"), 1, "Created", None, b"solo").await;

    // 2 + 3. Export + CBOR-encode to a REAL file on disk.
    let chunk = build_chunk(&src).await;
    std::fs::write(&chunk_file, &chunk).expect("write backup file");

    // 4. Reopen the file from scratch and decode it.
    let read_back = std::fs::read(&chunk_file).expect("read backup file");
    let sections = decode_chunk(&read_back).expect("decode chunk");
    assert_eq!(sections.len(), 2, "two stream sections round-tripped");

    // 5. Import into a FRESH fjall store (whole-chunk atomic restore).
    let dst = open_store(&dst_dir.path().join("dst-db"));
    let report = dst
        .import(&sections, identity_route, Atomicity::WholeChunk)
        .await
        .expect("import ok");
    assert!(report.all_complete(), "every stream restored");

    // 6. Streams are byte-equal modulo global_seq.
    assert_streams_equal_modulo_global_seq(&src, &dst, &sk("task-1")).await;
    assert_streams_equal_modulo_global_seq(&src, &dst, &sk("task-2")).await;

    // And global_seq is genuinely RE-STAMPED: the destination's $all sequence
    // is a fresh contiguous 1..=4, independent of the source's values.
    let mut dst_seqs: Vec<u64> = dst
        .read_all(GlobalSeq::INITIAL)
        .await
        .expect("read_all")
        .map(|r| r.expect("no error").global_seq().as_u64())
        .collect::<Vec<_>>()
        .await;
    dst_seqs.sort_unstable();
    assert_eq!(
        dst_seqs,
        vec![1, 2, 3, 4],
        "global_seq re-stamped on import"
    );
}

#[tokio::test]
async fn round_trip_survives_source_store_close_before_export() {
    // Lifecycle: write the source, CLOSE it, reopen, then export — the backup
    // must reflect the durable on-disk state, not in-memory residue.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    {
        let src = open_store(&db);
        append_one(&src, &sk("s"), 1, "E", None, b"one").await;
        append_one(&src, &sk("s"), 2, "E", None, b"two").await;
    }
    let reopened = open_store(&db);
    let chunk = build_chunk(&reopened).await;

    let dst = open_store(&dir.path().join("dst"));
    let sections = decode_chunk(&chunk).expect("decode");
    dst.import(&sections, identity_route, Atomicity::PerStream)
        .await
        .expect("import");
    assert_streams_equal_modulo_global_seq(&reopened, &dst, &sk("s")).await;
}

// ════════════════════════════════════════════════════════════════════════════
// Category 3 (defensive boundary): on-disk corruption surfaces correctly
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_disk_block_corruption_imports_as_stream_corrupt_not_malformed() {
    // Write a real chunk, corrupt a per-block BODY byte on disk (its crc32c will
    // no longer match), reopen, decode. A failed per-block checksum is a
    // non-fatal `ImportBlock::Corrupt` → `StreamOutcome::Corrupt`, NOT a
    // whole-chunk `ChunkError::Malformed` (which is for unparseable framing).
    let dir = tempfile::tempdir().unwrap();
    let chunk_file = dir.path().join("backup.nxch");

    let src = open_store(&dir.path().join("src"));
    append_one(&src, &sk("s"), 1, "E", None, b"good-one").await;
    append_one(&src, &sk("s"), 2, "E", None, b"good-two").await;
    let chunk = build_chunk(&src).await;
    std::fs::write(&chunk_file, &chunk).expect("write");

    // Corrupt the LAST byte on disk — the tail of the last block's body
    // (payload "good-two"), so its stored crc no longer matches. Framing stays
    // intact, so decode still parses the structure.
    let mut bytes = std::fs::read(&chunk_file).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&chunk_file, &bytes).expect("rewrite");

    let sections = decode_chunk(&std::fs::read(&chunk_file).expect("read"))
        .expect("framing still parses — only a block body is corrupt");
    assert_eq!(sections.len(), 1);

    // Import per-stream: v1 applies, the corrupt v2 block halts the stream.
    let dst = open_store(&dir.path().join("dst"));
    let report = dst
        .import(&sections, identity_route, Atomicity::PerStream)
        .await
        .expect("per-stream import never aborts the whole op");
    assert_eq!(report.streams().len(), 1);
    assert_eq!(
        report.streams()[0].outcome,
        StreamOutcome::Corrupt {
            reached: Some(Version::new(1).unwrap())
        },
        "good prefix applied, corrupt block halts the stream",
    );
    // The store holds exactly the good prefix [v1].
    let restored = collect_stream(&dst, &sk("s")).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].version(), Version::new(1).unwrap());
    assert_eq!(restored[0].payload(), b"good-one");
}

#[tokio::test]
async fn on_disk_block_corruption_aborts_whole_chunk_restore() {
    // The same on-disk block corruption under WHOLE-CHUNK atomicity must abort
    // the entire restore — nothing lands — and the fresh store stays empty
    // (cross-partition rollback on the persistent adapter).
    let dir = tempfile::tempdir().unwrap();
    let src = open_store(&dir.path().join("src"));
    append_one(&src, &sk("s"), 1, "E", None, b"good-one").await;
    append_one(&src, &sk("s"), 2, "E", None, b"good-two").await;
    let mut chunk = build_chunk(&src).await;
    let last = chunk.len() - 1;
    chunk[last] ^= 0xFF; // corrupt the last block's body

    let sections = decode_chunk(&chunk).expect("framing parses");
    let dst = open_store(&dir.path().join("dst"));
    let err = dst
        .import(&sections, identity_route, Atomicity::WholeChunk)
        .await
        .expect_err("whole-chunk corruption must abort");
    match err {
        ImportError::Aborted { stream, reason } => {
            assert_eq!(stream, sk("s"));
            assert_eq!(reason, AbortReason::Corrupt);
        }
        other => panic!("expected Aborted/Corrupt, got {other:?}"),
    }
    // Nothing landed: the fresh store has no streams at all.
    let ids: Vec<Vec<u8>> = dst
        .list_streams()
        .await
        .unwrap()
        .map(|r| r.unwrap().as_bytes().to_vec())
        .collect::<Vec<_>>()
        .await;
    assert!(ids.is_empty(), "aborted whole-chunk restore wrote nothing");
}

#[tokio::test]
async fn malformed_chunk_framing_is_a_decode_error_not_a_corrupt_block() {
    // Unparseable framing (bad magic / garbage) is `ChunkError::Malformed` at
    // decode time — a distinct failure domain from a per-block crc failure.
    let garbage = b"this is definitely not a valid nxch chunk header";
    match decode_chunk(garbage) {
        Err(ChunkError::Malformed(_)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }

    // A real header with a flipped magic byte is also Malformed.
    let dir = tempfile::tempdir().unwrap();
    let src = open_store(&dir.path().join("src"));
    append_one(&src, &sk("s"), 1, "E", None, b"x").await;
    let mut chunk = build_chunk(&src).await;
    chunk[3] ^= 0xFF; // the magic is the first bytes of the header map
    match decode_chunk(&chunk) {
        Err(ChunkError::Malformed(_)) => {}
        other => panic!("expected Malformed on bad magic, got {other:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Defensive boundary: non-injective route on the persistent adapter
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn non_injective_route_aborts_whole_chunk_no_corruption_on_disk() {
    // Two distinct origin streams routed to ONE target: whole-chunk import must
    // abort with nothing committed — never a concatenated [1,2,1,2] stream.
    let dir = tempfile::tempdir().unwrap();
    let src = open_store(&dir.path().join("src"));
    append_one(&src, &sk("o1"), 1, "E", None, b"a").await;
    append_one(&src, &sk("o1"), 2, "E", None, b"b").await;
    append_one(&src, &sk("o2"), 1, "E", None, b"c").await;
    append_one(&src, &sk("o2"), 2, "E", None, b"d").await;
    let chunk = build_chunk(&src).await;
    let sections = decode_chunk(&chunk).expect("decode");

    let dst = open_store(&dir.path().join("dst"));
    let to_same = |_origin: &[u8]| sk("merged");
    let err = dst
        .import(&sections, to_same, Atomicity::WholeChunk)
        .await
        .expect_err("non-injective route must abort");
    assert!(matches!(err, ImportError::Aborted { .. }));
    // The target never received the corrupt concatenation.
    assert!(collect_stream(&dst, &sk("merged")).await.is_empty());

    // Sanity: the source really did hold two distinct streams.
    let origins: BTreeSet<Vec<u8>> = src
        .list_streams()
        .await
        .unwrap()
        .map(|r| r.unwrap().as_bytes().to_vec())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect();
    assert_eq!(origins.len(), 2);
}
