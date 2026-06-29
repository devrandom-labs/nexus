#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use futures::StreamExt;
use nexus::Version;
use nexus_store::AppendError;
use nexus_store::StreamKey;
use nexus_store::pending_envelope;
use nexus_store::store::{GlobalSeq, RawEventStore};
use nexus_store::testing::InMemoryStore;

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

/// `InMemoryStore::append` assigns a store-global sequence to every event,
/// monotonically increasing across events in a single batch AND across
/// separate appends to different streams.
#[tokio::test]
async fn append_assigns_monotonic_global_seq_across_batches_and_streams() {
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

    // Read stream "a" back: seq 1 then seq 2.
    let mut stream_a = store
        .read_stream(&StreamKey::from_slice(b"a"), Version::INITIAL)
        .await
        .unwrap();
    let a1 = stream_a.next().await.unwrap().unwrap();
    assert_eq!(a1.event_type(), "A1");
    assert_eq!(a1.global_seq(), GlobalSeq::new(1).unwrap());
    let a2 = stream_a.next().await.unwrap().unwrap();
    assert_eq!(a2.event_type(), "A2");
    assert_eq!(a2.global_seq(), GlobalSeq::new(2).unwrap());
    assert!(stream_a.next().await.is_none());

    // Read stream "b" back: seq 3, proving the counter is store-global and
    // does not reset per stream.
    let mut stream_b = store
        .read_stream(&StreamKey::from_slice(b"b"), Version::INITIAL)
        .await
        .unwrap();
    let b1 = stream_b.next().await.unwrap().unwrap();
    assert_eq!(b1.event_type(), "B1");
    assert_eq!(b1.global_seq(), GlobalSeq::new(3).unwrap());
    assert!(stream_b.next().await.is_none());
}
