//! Smoke test for the Arc-based SharedSubscription on InMemoryStore.
//!
//! Proves the new shape works end-to-end: subscribe on `Arc<Store>`, append
//! events from another task, drain the cursor. The static-ness assertion is
//! the compile-time proof that the cursor outlives any caller scope.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::Arc;

use nexus::{Id, Version};
use nexus_store::envelope::pending_envelope;
use nexus_store::store::{RawEventStore, SharedSubscription};
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

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

#[tokio::test]
async fn shared_subscription_yields_appended_events() {
    let store = Arc::new(InMemoryStore::new());
    let id = TestId("s-1".to_owned());

    // Subscribe before any appends.
    let writer = Arc::clone(&store);
    let mut sub = store.subscribe(&id, None).await.unwrap();

    // Write one event from a separate task.
    let id_w = id.clone();
    let writer_handle = tokio::spawn(async move {
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"hello".to_vec())
            .build_without_metadata();
        writer.append(&id_w, None, &[env]).await.unwrap();
    });

    let env = sub.next().await.unwrap().unwrap();
    assert_eq!(env.version(), Version::new(1).unwrap());
    assert_eq!(env.event_type(), "Created");
    assert_eq!(env.payload(), b"hello");

    writer_handle.await.unwrap();
}

#[tokio::test]
async fn shared_subscription_cursor_is_static() {
    fn assert_static<T: 'static>(_: &T) {}
    let store = Arc::new(InMemoryStore::new());
    let id = TestId("s-1".to_owned());
    let sub = store.subscribe(&id, None).await.unwrap();
    // The whole point of the refactor: this assertion must compile.
    assert_static(&sub);
}
