#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for clarity and brevity"
)]
#![allow(
    clippy::expect_used,
    reason = "tests use expect for clarity and brevity"
)]
#![allow(clippy::str_to_string, reason = "tests use to_string/to_owned freely")]
#![allow(
    clippy::shadow_reuse,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::shadow_unrelated,
    reason = "tests shadow variables for readability"
)]
#![allow(clippy::panic, reason = "tests use panic for error propagation")]

use nexus::{Id, Version};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::testing::{InMemoryStore, InMemoryStream};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(&'static str);
impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl Id for TestId {}

// =============================================================================
// Helpers
// =============================================================================

fn make_envelopes(start: u64, count: u64) -> Vec<nexus_store::envelope::PendingEnvelope<()>> {
    (start..start + count)
        .map(|v| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("Event")
                .payload(v.to_le_bytes().to_vec())
                .build_without_metadata()
        })
        .collect()
}

/// Drain all remaining events from a stream into a Vec of (version, payload).
async fn collect_stream(stream: &mut InMemoryStream) -> Vec<(u64, Vec<u8>)> {
    let mut result = Vec::new();
    loop {
        let item = stream.next().await;
        match item {
            None => break,
            Some(Ok(env)) => {
                result.push((env.version().as_u64(), env.payload().to_vec()));
            }
            Some(Err(e)) => panic!("unexpected stream error: {e}"),
        }
    }
    result
}

// =============================================================================
// Integration tests
// =============================================================================

/// Append events 1-3, then append events 4-6 (with `expected_version=3`),
/// then read all and verify versions [1,2,3,4,5,6].
#[tokio::test]
async fn multi_batch_append_then_read_all() {
    let store = InMemoryStore::new();

    // Batch 1: events with versions 1, 2, 3
    let batch1 = make_envelopes(1, 3);
    store
        .append(&TestId("multi-batch-stream"), None, &batch1)
        .await
        .unwrap();

    // Batch 2: events with versions 4, 5, 6 — expected_version = 3
    let batch2 = make_envelopes(4, 3);
    store
        .append(&TestId("multi-batch-stream"), Version::new(3), &batch2)
        .await
        .unwrap();

    // Read all from the beginning (version 1 / INITIAL)
    let mut cursor = store
        .read_stream(&TestId("multi-batch-stream"), Version::INITIAL)
        .await
        .unwrap();
    let events = collect_stream(&mut cursor).await;

    let versions: Vec<u64> = events.iter().map(|(v, _)| *v).collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
}

/// Append 5 events, read from version 3, verify only versions [3,4,5] returned.
#[tokio::test]
async fn read_stream_from_version_filters_earlier() {
    let store = InMemoryStore::new();

    let envelopes = make_envelopes(1, 5);
    store
        .append(&TestId("filter-stream"), None, &envelopes)
        .await
        .unwrap();

    // Read starting from version 3
    let mut cursor = store
        .read_stream(&TestId("filter-stream"), Version::new(3).unwrap())
        .await
        .unwrap();
    let events = collect_stream(&mut cursor).await;

    let versions: Vec<u64> = events.iter().map(|(v, _)| *v).collect();
    assert_eq!(versions, vec![3, 4, 5]);
}

/// Append 1 event, then two writers both try to append with `expected_version=1`.
/// First succeeds, second should get conflict.
#[tokio::test]
async fn concurrent_append_detects_conflict() {
    let store = InMemoryStore::new();

    // Seed with 1 event
    let seed = make_envelopes(1, 1);
    store
        .append(&TestId("conflict-stream"), None, &seed)
        .await
        .unwrap();

    // Writer A: append event 2 with expected_version=1
    let writer_a = make_envelopes(2, 1);
    let result_a = store
        .append(&TestId("conflict-stream"), Version::new(1), &writer_a)
        .await;
    assert!(result_a.is_ok(), "writer A should succeed");

    // Writer B: also tries with expected_version=1 (stale), should conflict
    let writer_b = make_envelopes(2, 1);
    let result_b = store
        .append(&TestId("conflict-stream"), Version::new(1), &writer_b)
        .await;
    assert!(result_b.is_err(), "writer B should get a conflict error");

    // Verify the error is specifically a conflict
    let err = result_b.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("conflict"),
        "error should mention conflict, got: {err_msg}"
    );

    // Verify the stream only has the 2 events (seed + writer A)
    let mut cursor = store
        .read_stream(&TestId("conflict-stream"), Version::INITIAL)
        .await
        .unwrap();
    let events = collect_stream(&mut cursor).await;
    assert_eq!(events.len(), 2);
}

/// Append 1000 events, read all back, verify count and payload integrity.
#[tokio::test]
async fn large_batch_append_and_sequential_readback() {
    let store = InMemoryStore::new();
    let count: u64 = 1000;

    let envelopes = make_envelopes(1, count);
    store
        .append(&TestId("large-batch-stream"), None, &envelopes)
        .await
        .unwrap();

    let mut cursor = store
        .read_stream(&TestId("large-batch-stream"), Version::INITIAL)
        .await
        .unwrap();
    let events = collect_stream(&mut cursor).await;

    // Verify count
    assert_eq!(
        u64::try_from(events.len()).unwrap_or(0),
        count,
        "should have exactly {count} events"
    );

    // Verify payload integrity — each event's payload is its version as le bytes
    for (version, payload) in &events {
        let expected_payload = version.to_le_bytes().to_vec();
        assert_eq!(
            payload, &expected_payload,
            "payload mismatch at version {version}"
        );
    }

    // Verify sequential ordering
    let versions: Vec<u64> = events.iter().map(|(v, _)| *v).collect();
    let expected_versions: Vec<u64> = (1..=count).collect();
    assert_eq!(versions, expected_versions, "versions should be 1..=1000");
}

// =============================================================================
// Lifecycle edge cases
// =============================================================================

/// Reading from a version beyond the stream end yields empty.
#[tokio::test]
async fn read_from_future_version_returns_empty() {
    let store = InMemoryStore::new();
    let envelopes = make_envelopes(1, 3);
    store
        .append(&TestId("future-version-stream"), None, &envelopes)
        .await
        .unwrap();

    // Read from version 100 — no events exist at that version
    let mut cursor = store
        .read_stream(&TestId("future-version-stream"), Version::new(100).unwrap())
        .await
        .unwrap();
    assert!(
        cursor.next().await.is_none(),
        "should return empty for future version"
    );
}
