//! Cross-cutting tests for the wire-format payload-alignment invariant.
//!
//! Each test maps to one of the four CLAUDE.md-mandated categories:
//! sequence/protocol, lifecycle, defensive boundary, linearizability.
//! Together they assert that every `PersistedEnvelope` yielded by the
//! store has a payload pointer aligned to 16 bytes — the wire-format
//! invariant that `BytemuckCodec` and `RkyvCodec` (and any other
//! `align_to`-based zero-copy decoder) rely on.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    reason = "test readability: Arc clones into spawned tasks reuse names"
)]
#![allow(
    clippy::items_after_statements,
    reason = "tests inline const ET_LENGTHS at the only use site"
)]

use bytes::Bytes;
use futures::StreamExt;
use nexus::Version;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use nexus_store::{PendingEnvelope, StreamKey, pending_envelope};

const ALIGN: usize = 16;

fn build_envelope(version: u64, event_type: &'static str, payload_len: usize) -> PendingEnvelope {
    let payload = Bytes::from(vec![0xAA_u8; payload_len]);
    pending_envelope(Version::new(version).expect("version > 0"))
        .event_type(event_type)
        .payload(payload)
        .expect("valid payload")
        .build()
}

fn assert_payload_aligned(env: &nexus_store::PersistedEnvelope) {
    let ptr = env.payload().as_ptr() as usize;
    assert!(
        ptr.is_multiple_of(ALIGN),
        "payload pointer {ptr:#x} is not {ALIGN}-byte aligned for version {}",
        env.version().as_u64()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol — multiple appends, every yielded payload aligned
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn sequence_payloads_are_aligned_across_a_single_stream() {
    let store = InMemoryStore::new();
    let id = StreamKey::from_slice(b"seq");
    let envelopes: Vec<_> = (1..=20).map(|n| build_envelope(n, "E", 42)).collect();
    store.append(&id, None, &envelopes).await.unwrap();

    let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
    let mut count = 0_usize;
    while let Some(item) = stream.next().await {
        let env = item.unwrap();
        assert_payload_aligned(&env);
        count += 1;
    }
    assert_eq!(count, 20);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle — clone the store handle, read through a fresh cursor
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_clone_then_read_preserves_alignment() {
    let store = std::sync::Arc::new(InMemoryStore::new());
    let id = StreamKey::from_slice(b"lc");
    store
        .append(&id, None, &[build_envelope(1, "E", 7)])
        .await
        .unwrap();

    let cloned = std::sync::Arc::clone(&store);
    let mut stream = cloned.read_stream(&id, Version::INITIAL).await.unwrap();
    let env = stream.next().await.unwrap().unwrap();
    assert_payload_aligned(&env);
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive boundary — event_type lengths that cross 16-byte boundaries
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn boundary_event_type_lengths_around_alignment_boundary_all_aligned() {
    // The wire row layout (V2) is:
    //   [u8 frame_format_version][u32 schema_version]
    //   [u16 et_len][u32 meta_len][event_type][metadata?][padding][payload]
    //
    // Header fixed prefix is 11 bytes (u8 + u32 + u16 + u32). Then the
    // event_type and metadata extend the pre-padding header; the padding
    // takes the payload start up to the next 16-byte boundary.
    //
    // We exercise event_type lengths around the 16-byte boundaries to
    // confirm the padding logic is correct at the seam.
    let store = InMemoryStore::new();
    let id = StreamKey::from_slice(b"boundary");
    // Padding kicks in at header_len % 16 != 0. We pick a range of
    // event_type lengths that put the header at various positions inside
    // a 16-byte window.
    const ET_LENGTHS: &[usize] = &[0, 1, 6, 13, 14, 15, 16, 17, 30, 100, 1024];
    let mut event_types: Vec<&'static str> = Vec::with_capacity(ET_LENGTHS.len());
    for len in ET_LENGTHS {
        event_types.push(Box::leak("E".repeat(*len).into_boxed_str()));
    }
    let envelopes: Vec<_> = (1_u64..)
        .zip(event_types.iter())
        .map(|(v, et)| build_envelope(v, et, 42))
        .collect();
    store.append(&id, None, &envelopes).await.unwrap();

    let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
    while let Some(item) = stream.next().await {
        let env = item.unwrap();
        assert_payload_aligned(&env);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Linearizability — concurrent writer + reader, every payload aligned
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn linearizability_concurrent_writer_aligned_payloads() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let store = Arc::new(InMemoryStore::new());
    let id = StreamKey::from_slice(b"lin");

    // Pre-seed one event so the reader has something to consume.
    store
        .append(&id, None, &[build_envelope(1, "E", 16)])
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(2));

    let writer = {
        let store = Arc::clone(&store);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            for n in 2_u64..=50 {
                let expected = Version::new(n - 1);
                store
                    .append(&id, expected, &[build_envelope(n, "E", 16)])
                    .await
                    .unwrap();
            }
        })
    };

    let reader = {
        let store = Arc::clone(&store);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            for _ in 0..30 {
                let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
                while let Some(item) = stream.next().await {
                    let env = item.unwrap();
                    assert_payload_aligned(&env);
                }
            }
        })
    };

    let (w, r) = tokio::join!(writer, reader);
    w.unwrap();
    r.unwrap();
}
