#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use futures::StreamExt;
use nexus::Version;
use nexus_store::AppendError;
use nexus_store::StreamKey;
use nexus_store::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::{InMemoryAllPos, InMemoryStore};

#[tokio::test]
async fn append_conflict_truncates_overlong_stream_id_with_ellipsis() {
    let store = InMemoryStore::new();
    // An overlong id so the conflict label exceeds the 64-byte `ErrorId` cap.
    let long = StreamKey::from_slice("y".repeat(200).as_bytes());
    let env = pending_envelope(Version::new(1).unwrap())
        .event_type("E")
        .payload(b"p".to_vec())
        .unwrap()
        .build();
    // New stream + Some(expected) → conflict carrying the truncated id label.
    let err = store
        .append(&long, Version::new(1), &[env])
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
        // AppendError is #[non_exhaustive] (#209): Store and any future variant
        // collapse into the catch-all — only Conflict is expected here.
        other => panic!("expected Conflict, got: {other}"),
    }
}

#[tokio::test]
async fn append_and_read_back() {
    let store = InMemoryStore::new();
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("TestEvent")
        .payload(b"hello".to_vec())
        .expect("valid payload")
        .build();
    store
        .append(&StreamKey::from_slice(b"stream-1"), None, &[envelope])
        .await
        .unwrap();

    let mut stream = store
        .read_stream(&StreamKey::from_slice(b"stream-1"), Version::INITIAL)
        .await
        .unwrap();
    let env = stream.next().await.unwrap().unwrap();
    assert_eq!(env.event_type(), "TestEvent");
    assert_eq!(env.payload(), b"hello");
    assert_eq!(env.version(), Version::INITIAL);
    assert_eq!(env.schema_version(), 1);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn read_from_version_filters_correctly() {
    let store = InMemoryStore::new();

    let envelopes = vec![
        pending_envelope(Version::new(1).unwrap())
            .event_type("E1")
            .payload(b"one".to_vec())
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("E2")
            .payload(b"two".to_vec())
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(3).unwrap())
            .event_type("E3")
            .payload(b"three".to_vec())
            .expect("valid payload")
            .build(),
    ];
    store
        .append(&StreamKey::from_slice(b"s1"), None, &envelopes)
        .await
        .unwrap();

    // Read from version 2 -- should get events 2 and 3.
    let mut stream = store
        .read_stream(&StreamKey::from_slice(b"s1"), Version::new(2).unwrap())
        .await
        .unwrap();

    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.event_type(), "E2");
    assert_eq!(e1.version(), Version::new(2).unwrap());

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.event_type(), "E3");
    assert_eq!(e2.version(), Version::new(3).unwrap());

    assert!(stream.next().await.is_none());
}

/// `InMemoryStore::append` assigns a store-global `$all` position to every
/// event, monotonically increasing across events in a single batch AND across
/// separate appends to different streams. The position rides on the `$all`
/// read tag (a per-stream event carries none).
#[tokio::test]
async fn append_assigns_monotonic_all_position_across_batches_and_streams() {
    let store = InMemoryStore::new();

    // First append: a two-event batch on stream "a". Events get seq 1, 2.
    let batch_a = vec![
        pending_envelope(Version::new(1).unwrap())
            .event_type("A1")
            .payload(b"a1".to_vec())
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("A2")
            .payload(b"a2".to_vec())
            .expect("valid payload")
            .build(),
    ];
    store
        .append(&StreamKey::from_slice(b"a"), None, &batch_a)
        .await
        .unwrap();

    // Second append: a single event on a *different* stream "b". Continues
    // from where stream "a" left off — seq 3, not reset to 1.
    let batch_b = vec![
        pending_envelope(Version::new(1).unwrap())
            .event_type("B1")
            .payload(b"b1".to_vec())
            .expect("valid payload")
            .build(),
    ];
    store
        .append(&StreamKey::from_slice(b"b"), None, &batch_b)
        .await
        .unwrap();

    // Read `$all` back: positions 1,2 (stream "a"), then 3 (stream "b") —
    // proving the counter is store-global and does not reset per stream. The
    // position rides on the tag, not the (position-free) envelope.
    let mut all = store.read_all(None).await.unwrap();
    let (p1, a1) = all.next().await.unwrap().unwrap();
    assert_eq!(a1.event_type(), "A1");
    assert_eq!(p1, InMemoryAllPos::new(1).unwrap());
    let (p2, a2) = all.next().await.unwrap().unwrap();
    assert_eq!(a2.event_type(), "A2");
    assert_eq!(p2, InMemoryAllPos::new(2).unwrap());
    let (p3, b1) = all.next().await.unwrap().unwrap();
    assert_eq!(b1.event_type(), "B1");
    assert_eq!(p3, InMemoryAllPos::new(3).unwrap());
    assert!(all.next().await.is_none());
}
