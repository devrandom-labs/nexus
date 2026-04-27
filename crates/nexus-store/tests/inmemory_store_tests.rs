#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::{Id, Version};
use nexus_store::pending_envelope;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(&'static str);
impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
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

#[tokio::test]
async fn append_and_read_back() {
    let store = InMemoryStore::new();
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("TestEvent")
        .payload(b"hello".to_vec())
        .build_without_metadata();
    store
        .append(&TestId("stream-1"), None, &[envelope])
        .await
        .unwrap();

    let mut stream = store
        .read_stream(&TestId("stream-1"), Version::INITIAL)
        .await
        .unwrap();
    let env = stream.next().await.unwrap().unwrap();
    assert_eq!(env.event_type(), "TestEvent");
    assert_eq!(env.payload(), b"hello");
    assert_eq!(env.version(), Version::INITIAL);
    assert_eq!(env.schema_version(), 1);
    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn read_from_version_filters_correctly() {
    let store = InMemoryStore::new();

    let envelopes = vec![
        pending_envelope(Version::new(1).unwrap())
            .event_type("E1")
            .payload(b"one".to_vec())
            .build_without_metadata(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("E2")
            .payload(b"two".to_vec())
            .build_without_metadata(),
        pending_envelope(Version::new(3).unwrap())
            .event_type("E3")
            .payload(b"three".to_vec())
            .build_without_metadata(),
    ];
    store.append(&TestId("s1"), None, &envelopes).await.unwrap();

    // Read from version 2 -- should get events 2 and 3.
    let mut stream = store
        .read_stream(&TestId("s1"), Version::new(2).unwrap())
        .await
        .unwrap();

    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.event_type(), "E2");
    assert_eq!(e1.version(), Version::new(2).unwrap());

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.event_type(), "E3");
    assert_eq!(e2.version(), Version::new(3).unwrap());

    assert!(stream.next().await.unwrap().is_none());
}
