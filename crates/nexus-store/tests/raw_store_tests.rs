use nexus::StreamId;
use nexus::Version;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;

#[allow(clippy::unwrap_used, reason = "test helper")]
fn sid(s: &str) -> StreamId {
    StreamId::from_persisted(s).unwrap()
}

#[tokio::test]
async fn raw_store_append_and_read() {
    let store = InMemoryStore::new();

    let envelopes = vec![
        pending_envelope(sid("s1"))
            .version(Version::from_persisted(1))
            .event_type("Created")
            .payload(vec![1])
            .build_without_metadata(),
        pending_envelope(sid("s1"))
            .version(Version::from_persisted(2))
            .event_type("Updated")
            .payload(vec![2])
            .build_without_metadata(),
    ];

    store
        .append(&sid("s1"), Version::INITIAL, &envelopes)
        .await
        .unwrap();

    let mut stream = store
        .read_stream(&sid("s1"), Version::INITIAL)
        .await
        .unwrap();

    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.event_type(), "Created");
    }

    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.event_type(), "Updated");
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn raw_store_optimistic_concurrency() {
    let store = InMemoryStore::new();

    let e1 = vec![
        pending_envelope(sid("s1"))
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
    ];
    store
        .append(&sid("s1"), Version::INITIAL, &e1)
        .await
        .unwrap();

    // Wrong expected version -- should fail.
    let e2 = vec![
        pending_envelope(sid("s1"))
            .version(Version::from_persisted(2))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
    ];
    let result = store.append(&sid("s1"), Version::INITIAL, &e2).await;
    assert!(result.is_err());
}
