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
#![allow(
    clippy::significant_drop_tightening,
    reason = "lock guard lifetime is fine in test adapters"
)]

use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use std::collections::HashMap;
use tokio::sync::Mutex;

// =============================================================================
// In-memory adapter (copied from raw_store_tests.rs)
// =============================================================================

/// Row stored per event: (version, `event_type`, payload).
type StoredRow = (u64, String, Vec<u8>);

/// Minimal in-memory adapter for testing `RawEventStore`.
struct InMemoryRawStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemoryRawStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

struct InMemoryStream {
    events: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl EventStream for InMemoryStream {
    type Error = TestError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            &row.3,
            (),
        )))
    }
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("concurrency conflict")]
    Conflict,
}

impl RawEventStore for InMemoryRawStore {
    type Error = TestError;
    type Stream<'a>
        = InMemoryStream
    where
        Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_version != expected_version.as_u64() {
            return Err(TestError::Conflict);
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        drop(guard);
        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|(v, _, _)| *v >= from.as_u64())
                    .map(|(v, t, p)| (stream_id.to_owned(), *v, t.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(InMemoryStream { events, pos: 0 })
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn make_envelopes(stream_id: &str, start: u64, count: u64) -> Vec<PendingEnvelope<()>> {
    (start..start + count)
        .map(|v| {
            pending_envelope(stream_id.into())
                .version(Version::from_persisted(v))
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
    let store = InMemoryRawStore::new();
    let sid = "multi-batch-stream";

    // Batch 1: events with versions 1, 2, 3
    let batch1 = make_envelopes(sid, 1, 3);
    store.append(sid, Version::INITIAL, &batch1).await.unwrap();

    // Batch 2: events with versions 4, 5, 6 — expected_version = 3
    let batch2 = make_envelopes(sid, 4, 3);
    store
        .append(sid, Version::from_persisted(3), &batch2)
        .await
        .unwrap();

    // Read all from the beginning (version 0 / INITIAL)
    let mut stream = store.read_stream(sid, Version::INITIAL).await.unwrap();
    let events = collect_stream(&mut stream).await;

    let versions: Vec<u64> = events.iter().map(|(v, _)| *v).collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
}

/// Append 5 events, read from version 3, verify only versions [3,4,5] returned.
#[tokio::test]
async fn read_stream_from_version_filters_earlier() {
    let store = InMemoryRawStore::new();
    let sid = "filter-stream";

    let envelopes = make_envelopes(sid, 1, 5);
    store
        .append(sid, Version::INITIAL, &envelopes)
        .await
        .unwrap();

    // Read starting from version 3
    let mut stream = store
        .read_stream(sid, Version::from_persisted(3))
        .await
        .unwrap();
    let events = collect_stream(&mut stream).await;

    let versions: Vec<u64> = events.iter().map(|(v, _)| *v).collect();
    assert_eq!(versions, vec![3, 4, 5]);
}

/// Append 1 event, then two writers both try to append with `expected_version=1`.
/// First succeeds, second should get conflict.
#[tokio::test]
async fn concurrent_append_detects_conflict() {
    let store = InMemoryRawStore::new();
    let sid = "conflict-stream";

    // Seed with 1 event
    let seed = make_envelopes(sid, 1, 1);
    store.append(sid, Version::INITIAL, &seed).await.unwrap();

    // Writer A: append event 2 with expected_version=1
    let writer_a = make_envelopes(sid, 2, 1);
    let result_a = store
        .append(sid, Version::from_persisted(1), &writer_a)
        .await;
    assert!(result_a.is_ok(), "writer A should succeed");

    // Writer B: also tries with expected_version=1 (stale), should conflict
    let writer_b = make_envelopes(sid, 2, 1);
    let result_b = store
        .append(sid, Version::from_persisted(1), &writer_b)
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
    let mut stream = store.read_stream(sid, Version::INITIAL).await.unwrap();
    let events = collect_stream(&mut stream).await;
    assert_eq!(events.len(), 2);
}

/// Append 1000 events, read all back, verify count and payload integrity.
#[tokio::test]
async fn large_batch_append_and_sequential_readback() {
    let store = InMemoryRawStore::new();
    let sid = "large-batch-stream";
    let count: u64 = 1000;

    let envelopes = make_envelopes(sid, 1, count);
    store
        .append(sid, Version::INITIAL, &envelopes)
        .await
        .unwrap();

    let mut stream = store.read_stream(sid, Version::INITIAL).await.unwrap();
    let events = collect_stream(&mut stream).await;

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
