#![cfg(feature = "testing")]

use nexus::Version;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;

#[tokio::test]
async fn append_and_read_back() {
    let store = InMemoryStore::new();
    let envelope = pending_envelope("stream-1".into())
        .version(Version::from_persisted(1))
        .event_type("TestEvent")
        .payload(b"hello".to_vec())
        .build_without_metadata();
    store
        .append("stream-1", Version::INITIAL, &[envelope])
        .await
        .unwrap();

    let mut stream = store
        .read_stream("stream-1", Version::INITIAL)
        .await
        .unwrap();
    let env = stream.next().await.unwrap().unwrap();
    assert_eq!(env.event_type(), "TestEvent");
    assert_eq!(env.payload(), b"hello");
    assert_eq!(env.version(), Version::from_persisted(1));
    assert_eq!(env.schema_version(), 1);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn optimistic_concurrency_rejects_wrong_version() {
    let store = InMemoryStore::new();

    // Append one event at INITIAL.
    let e1 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E1")
        .payload(vec![])
        .build_without_metadata();
    store.append("s1", Version::INITIAL, &[e1]).await.unwrap();

    // Try to append at INITIAL again -- should fail.
    let e2 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E2")
        .payload(vec![])
        .build_without_metadata();
    let result = store.append("s1", Version::INITIAL, &[e2]).await;
    assert!(result.is_err(), "expected conflict error");
}

#[tokio::test]
async fn read_empty_stream_returns_no_events() {
    let store = InMemoryStore::new();
    let mut stream = store
        .read_stream("nonexistent", Version::INITIAL)
        .await
        .unwrap();
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn read_from_version_filters_correctly() {
    let store = InMemoryStore::new();

    let envelopes = vec![
        pending_envelope("s1".into())
            .version(Version::from_persisted(1))
            .event_type("E1")
            .payload(b"one".to_vec())
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(2))
            .event_type("E2")
            .payload(b"two".to_vec())
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(3))
            .event_type("E3")
            .payload(b"three".to_vec())
            .build_without_metadata(),
    ];
    store
        .append("s1", Version::INITIAL, &envelopes)
        .await
        .unwrap();

    // Read from version 2 -- should get events 2 and 3.
    let mut stream = store
        .read_stream("s1", Version::from_persisted(2))
        .await
        .unwrap();

    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.event_type(), "E2");
    assert_eq!(e1.version(), Version::from_persisted(2));

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.event_type(), "E3");
    assert_eq!(e2.version(), Version::from_persisted(3));

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn sequential_version_validation_on_append() {
    let store = InMemoryStore::new();

    // Valid: envelope with version 1 at expected 0.
    let e1 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E1")
        .payload(vec![])
        .build_without_metadata();
    store.append("s1", Version::INITIAL, &[e1]).await.unwrap();

    // Invalid: envelope with version 5 at expected 1 (should be version 2).
    let e_bad = pending_envelope("s1".into())
        .version(Version::from_persisted(5))
        .event_type("E2")
        .payload(vec![])
        .build_without_metadata();
    let result = store
        .append("s1", Version::from_persisted(1), &[e_bad])
        .await;
    assert!(result.is_err(), "expected version validation error");
}
