//! Subscription tests for the fjall event store adapter.
//!
//! Covers 4 cross-cutting test categories:
//! 1. Sequence/Protocol — catch-up then live, checkpoint resume
//! 2. Lifecycle — drop/resubscribe, close/reopen/subscribe
//! 3. Defensive Boundary — nonexistent stream, beyond-head subscribe
//! 4. Linearizability — concurrent append+subscribe, multiple subscribers

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::panic, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]
#![allow(clippy::shadow_reuse, reason = "tests")]

use std::sync::Arc;
use std::time::Duration;

use nexus::{Id, Version};
use nexus_fjall::FjallStore;
use nexus_store::PendingEnvelope;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::{CheckpointStore, EventStream, RawEventStore, Subscription};
use tokio::time::timeout;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl TestId {
    fn new(s: &str) -> Self {
        Self(s.to_owned())
    }
}

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

impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

fn make_envelope(version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope<()> {
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

/// Helper: append a single event to a stream, with expected version.
async fn append_one(
    store: &FjallStore,
    id: &TestId,
    version: u64,
    expected: Option<Version>,
    event_type: &'static str,
) {
    let envelope = make_envelope(version, event_type, format!("payload-{version}").as_bytes());
    store.append(id, expected, &[envelope]).await.unwrap();
}

/// Timeout duration for operations that should complete quickly.
const TIMEOUT: Duration = Duration::from_secs(2);

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn subscribe_catchup_then_live() {
    let (store, _dir) = temp_store();
    let id = TestId::new("stream-1");

    // Pre-populate 2 events.
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;

    // Subscribe from the beginning (None = start from version 1).
    let mut stream = store.subscribe(&id, None).await.unwrap();

    // Read catch-up event 1.
    let env1 = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env1.event_type(), "E1");
    assert_eq!(env1.version(), Version::new(1).unwrap());

    // Read catch-up event 2.
    let env2 = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env2.event_type(), "E2");
    assert_eq!(env2.version(), Version::new(2).unwrap());

    // Append a 3rd event (live).
    append_one(&store, &id, 3, Version::new(2), "E3").await;

    // Read the live event.
    let env3 = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env3.event_type(), "E3");
    assert_eq!(env3.version(), Version::new(3).unwrap());
}

#[tokio::test]
async fn subscribe_from_checkpoint() {
    let (store, _dir) = temp_store();
    let id = TestId::new("stream-1");

    // Pre-populate 3 events.
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;
    append_one(&store, &id, 3, Version::new(2), "E3").await;

    // Subscribe from version 2 (should yield events AFTER version 2, i.e., event 3).
    let mut stream = store
        .subscribe(&id, Some(Version::new(2).unwrap()))
        .await
        .unwrap();

    let env = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env.event_type(), "E3");
    assert_eq!(env.version(), Version::new(3).unwrap());
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_and_resubscribe() {
    let (store, _dir) = temp_store();
    let id = TestId::new("stream-1");

    // Append event 1.
    append_one(&store, &id, 1, None, "E1").await;

    // Subscribe, read event, note checkpoint version, drop.
    let checkpoint = {
        let mut sub_stream = store.subscribe(&id, None).await.unwrap();
        let first_env = timeout(TIMEOUT, sub_stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first_env.version(), Version::new(1).unwrap());
        first_env.version()
    };

    // Append more events while subscription is dropped.
    append_one(&store, &id, 2, Version::new(1), "E2").await;
    append_one(&store, &id, 3, Version::new(2), "E3").await;

    // Re-subscribe from saved checkpoint.
    let mut stream = store.subscribe(&id, Some(checkpoint)).await.unwrap();

    // Should get events 2 and 3 (after the checkpoint).
    let env2 = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env2.event_type(), "E2");
    assert_eq!(env2.version(), Version::new(2).unwrap());

    let env3 = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env3.event_type(), "E3");
    assert_eq!(env3.version(), Version::new(3).unwrap());
}

#[tokio::test]
async fn write_close_reopen_subscribe() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    let id = TestId::new("stream-1");

    // Phase 1: Open, append events, close.
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        append_one(&store, &id, 1, None, "E1").await;
        append_one(&store, &id, 2, Version::new(1), "E2").await;
        // Store dropped here — flushes to disk.
    }

    // Phase 2: Reopen and subscribe — verify all events via catch-up.
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let mut stream = store.subscribe(&id, None).await.unwrap();

        let env1 = timeout(TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(env1.event_type(), "E1");
        assert_eq!(env1.version(), Version::new(1).unwrap());

        let env2 = timeout(TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(env2.event_type(), "E2");
        assert_eq!(env2.version(), Version::new(2).unwrap());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn subscribe_to_nonexistent_stream_waits() {
    let (store, _dir) = temp_store();
    let id = TestId::new("ghost-stream");

    let mut stream = store.subscribe(&id, None).await.unwrap();

    // next() should block because the stream doesn't exist yet.
    let result = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;
    assert!(result.is_err(), "expected timeout, but got an event");

    // Now append to the stream.
    append_one(&store, &id, 1, None, "E1").await;

    // Should receive the event.
    let env = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env.event_type(), "E1");
    assert_eq!(env.version(), Version::new(1).unwrap());
}

#[tokio::test]
async fn subscribe_from_beyond_head() {
    let (store, _dir) = temp_store();
    let id = TestId::new("stream-1");

    // Append 2 events (head is at version 2).
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;

    // Subscribe from version 5 — beyond the current head.
    let mut stream = store
        .subscribe(&id, Some(Version::new(5).unwrap()))
        .await
        .unwrap();

    // Should block — no events at version 6+.
    let result = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;
    assert!(result.is_err(), "expected timeout, but got an event");

    // Append events up to and beyond version 5.
    append_one(&store, &id, 3, Version::new(2), "E3").await;
    append_one(&store, &id, 4, Version::new(3), "E4").await;
    append_one(&store, &id, 5, Version::new(4), "E5").await;
    append_one(&store, &id, 6, Version::new(5), "E6").await;

    // Should receive version 6 (first event AFTER from=5).
    let env = timeout(TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env.event_type(), "E6");
    assert_eq!(env.version(), Version::new(6).unwrap());
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Linearizability/Isolation Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn concurrent_append_and_subscribe() {
    let (store, _dir) = temp_store();
    let store = Arc::new(store);
    let id = TestId::new("concurrent-stream");
    let event_count: u64 = 50;

    let mut stream = store.subscribe(&id, None).await.unwrap();

    // Spawn a task that appends events sequentially.
    let writer_store = Arc::clone(&store);
    let writer_id = id.clone();
    let writer = tokio::spawn(async move {
        for i in 1..=event_count {
            let expected = if i == 1 {
                None
            } else {
                Version::new(i.checked_sub(1).unwrap())
            };
            append_one(&writer_store, &writer_id, i, expected, "ConcurrentEvent").await;
            // Yield to allow reader to interleave.
            tokio::task::yield_now().await;
        }
    });

    // Read all events from the subscriber.
    for expected_version in 1..=event_count {
        let env = timeout(TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            env.version(),
            Version::new(expected_version).unwrap(),
            "expected version {expected_version}, got {}",
            env.version()
        );
        assert_eq!(env.event_type(), "ConcurrentEvent");
    }

    writer.await.unwrap();
}

/// Append during catch-up phase doesn't lose events.
#[tokio::test]
async fn append_during_catchup_no_loss() {
    let (store, _dir) = temp_store();
    let id = TestId::new("stream-1");

    // Pre-populate 20 events.
    for i in 1..=20u64 {
        let expected = if i == 1 { None } else { Version::new(i - 1) };
        append_one(&store, &id, i, expected, "Prepop").await;
    }

    let mut stream = store.subscribe(&id, None).await.unwrap();

    // Read first 5 events (mid-catch-up).
    for expected_v in 1..=5u64 {
        let env = timeout(TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(env.version(), Version::new(expected_v).unwrap());
    }

    // Append a new event while we're mid-catch-up.
    append_one(&store, &id, 21, Version::new(20), "Live").await;

    // Continue reading: should get events 6-20 (remaining catch-up) then 21 (live).
    for expected_v in 6..=21u64 {
        let env = timeout(TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(env.version(), Version::new(expected_v).unwrap());
    }
}

#[tokio::test]
async fn multiple_subscribers_same_stream() {
    let (store, _dir) = temp_store();
    let id = TestId::new("shared-stream");

    // Two subscribers to the same stream.
    let mut sub1 = store.subscribe(&id, None).await.unwrap();
    let mut sub2 = store.subscribe(&id, None).await.unwrap();

    // Append one event.
    append_one(&store, &id, 1, None, "SharedEvent").await;

    // Both subscribers should see the event.
    let env1 = timeout(TIMEOUT, sub1.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env1.event_type(), "SharedEvent");
    assert_eq!(env1.version(), Version::new(1).unwrap());

    let env2 = timeout(TIMEOUT, sub2.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(env2.event_type(), "SharedEvent");
    assert_eq!(env2.version(), Version::new(1).unwrap());
}

// ═══════════════════════════════════════════════════════════════════════════
// CheckpointStore tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn checkpoint_load_unknown_returns_none() {
    let (store, _dir) = temp_store();
    let result = store.load(&TestId::new("nonexistent")).await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn checkpoint_save_load_roundtrip() {
    let (store, _dir) = temp_store();
    store
        .save(&TestId::new("sub-1"), Version::new(42).unwrap())
        .await
        .unwrap();
    let loaded = store.load(&TestId::new("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(Version::new(42).unwrap()));
}

#[tokio::test]
async fn checkpoint_save_overwrites() {
    let (store, _dir) = temp_store();
    store
        .save(&TestId::new("sub-1"), Version::new(1).unwrap())
        .await
        .unwrap();
    store
        .save(&TestId::new("sub-1"), Version::new(5).unwrap())
        .await
        .unwrap();
    let loaded = store.load(&TestId::new("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(Version::new(5).unwrap()));
}

#[tokio::test]
async fn checkpoint_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        store
            .save(&TestId::new("sub-1"), Version::new(10).unwrap())
            .await
            .unwrap();
        drop(store);
    }

    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let loaded = store.load(&TestId::new("sub-1")).await.unwrap();
        assert_eq!(loaded, Some(Version::new(10).unwrap()));
    }
}
