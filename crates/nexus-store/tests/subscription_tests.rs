#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::time::Duration;

use futures::StreamExt;
use nexus::{Id, Version};
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use nexus_store::{Store, Subscription, pending_envelope};
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

/// Helper: build a pending envelope with a given version and event type.
fn make_envelope(version: u64, event_type: &'static str) -> nexus_store::PendingEnvelope {
    pending_envelope(Version::new(version).unwrap())
        .event_type(event_type)
        .payload(format!("payload-{version}").into_bytes())
        .expect("valid payload")
        .build()
}

/// Helper: append a single event to a stream, with expected version.
async fn append_one(
    store: &Store<InMemoryStore>,
    id: &TestId,
    version: u64,
    expected: Option<Version>,
    event_type: &'static str,
) {
    let envelope = make_envelope(version, event_type);
    store
        .append(
            &nexus_store::StreamKey::from_slice(id.as_ref()),
            expected,
            &[envelope],
        )
        .await
        .unwrap();
}

/// Timeout duration for operations that should complete quickly.
const TIMEOUT: Duration = Duration::from_secs(2);

// PR2 (#208): the public `subscribe` / `subscribe_all` return types are bound
// to the re-exported `Stream` trait (`futures_core::Stream`), not the churning
// `futures` umbrella crate. This compiles only if the returned cursors satisfy
// that trait — a static guard on the public stream surface.
#[tokio::test]
async fn subscribe_returns_a_reexported_stream() {
    fn assert_stream<T: nexus_store::Stream>(_: &T) {}

    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-static");

    let per_stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    assert_stream(&per_stream);

    let all = Subscription::new(&store).subscribe_all(None).unwrap();
    assert_stream(&all);
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn subscribe_catchup_then_live() {
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Pre-populate 2 events.
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;

    // Subscribe from the beginning (None = start from version 1).
    let stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    futures::pin_mut!(stream);

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
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Pre-populate 3 events.
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;
    append_one(&store, &id, 3, Version::new(2), "E3").await;

    // Subscribe from version 2 (should yield events AFTER version 2, i.e., event 3).
    let stream = Subscription::new(&store)
        .subscribe(&id, Some(Version::new(2).unwrap()))
        .unwrap();
    futures::pin_mut!(stream);

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
async fn drop_and_resubscribe_from_position() {
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Append event 1.
    append_one(&store, &id, 1, None, "E1").await;

    // Subscribe, read event, capture position, drop.
    let position = {
        let sub_stream = Subscription::new(&store).subscribe(&id, None).unwrap();
        futures::pin_mut!(sub_stream);
        let first_env = timeout(TIMEOUT, sub_stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first_env.version(), Version::new(1).unwrap());
        first_env.version()
    };
    assert_eq!(position, Version::new(1).unwrap());

    // Append more events while subscription is dropped.
    append_one(&store, &id, 2, Version::new(1), "E2").await;
    append_one(&store, &id, 3, Version::new(2), "E3").await;

    // Re-subscribe from the captured position.
    let stream = Subscription::new(&store)
        .subscribe(&id, Some(position))
        .unwrap();
    futures::pin_mut!(stream);

    // Should get events 2 and 3 (after the position).
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
async fn catchup_events_appended_before_subscribe() {
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Append 2 events before any subscribe.
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;

    // Subscribe and verify both arrive as catch-up.
    let stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    futures::pin_mut!(stream);

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

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn subscribe_to_nonexistent_stream_waits() {
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("ghost-stream");

    let stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    futures::pin_mut!(stream);

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

/// Subscribe with `from` version beyond current stream head.
#[tokio::test]
async fn subscribe_from_beyond_head() {
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Append 2 events (head is at version 2).
    append_one(&store, &id, 1, None, "E1").await;
    append_one(&store, &id, 2, Version::new(1), "E2").await;

    // Subscribe from version 5 — beyond the current head.
    let stream = Subscription::new(&store)
        .subscribe(&id, Some(Version::new(5).unwrap()))
        .unwrap();
    futures::pin_mut!(stream);

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
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("concurrent-stream");
    let event_count: u64 = 50;

    let stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    futures::pin_mut!(stream);

    // Spawn a task that appends events sequentially.
    let writer_store = store.clone();
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
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("stream-1");

    // Pre-populate 20 events.
    for i in 1..=20u64 {
        let expected = if i == 1 { None } else { Version::new(i - 1) };
        append_one(&store, &id, i, expected, "Prepop").await;
    }

    let stream = Subscription::new(&store).subscribe(&id, None).unwrap();
    futures::pin_mut!(stream);

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
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("shared-stream");

    // Two subscribers to the same stream.
    let sub = Subscription::new(&store);
    let sub1 = sub.subscribe(&id, None).unwrap();
    let sub2 = sub.subscribe(&id, None).unwrap();
    futures::pin_mut!(sub1, sub2);

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
// 5. Static-ness compile-time guarantee
// ═══════════════════════════════════════════════════════════════════════════

/// The cursor returned by `subscribe` must be `'static` — the whole point
/// of the Arc-based subscription shape. If this assertion compiles, the
/// cursor outlives any caller scope and can be spawned across tasks.
#[tokio::test]
async fn subscription_cursor_is_static() {
    fn assert_static<T: 'static>(_: &T) {}
    let store = Store::new(InMemoryStore::new());
    let id = TestId::new("s-1");
    let sub = Subscription::new(&store).subscribe(&id, None).unwrap();
    assert_static(&sub);
}
