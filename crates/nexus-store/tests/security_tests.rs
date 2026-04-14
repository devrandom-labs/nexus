//! Security & reliability tests for `nexus-store`.
//!
//! Reproduces vulnerabilities found during mission-critical audit.
//! Each test documents a specific threat and verifies the store handles it safely.

#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for clarity and brevity"
)]
#![allow(
    clippy::shadow_unrelated,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::match_same_arms,
    reason = "exhaustive match documents known variants"
)]
#![allow(
    clippy::explicit_counter_loop,
    reason = "counter loop is clearer for version validation"
)]
#![allow(clippy::panic, reason = "tests use panic for assertions")]
#![allow(
    clippy::drop_non_drop,
    reason = "explicit drops for lending iterator documentation"
)]
#![allow(clippy::str_to_string, reason = "tests use to_string/to_owned freely")]

use nexus::{Id, Version};
use nexus_store::ToStreamLabel;
use nexus_store::error::StoreError;
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
impl Id for TestId {}

// =============================================================================
// C2: Builder accepts any String — content validation is the facade's job
// =============================================================================

#[test]
fn c2_builder_accepts_empty_event_type() {
    // Same rationale: the builder takes &'static str, so "" is valid.
    // The EventStore facade must reject empty event_type at runtime.
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("")
        .payload(vec![1])
        .build_without_metadata();
    assert_eq!(envelope.event_type(), "");
}

// =============================================================================
// H1: RawEventStore::append accepts empty envelope slice
// =============================================================================

#[tokio::test]
async fn h1_append_empty_envelopes_is_noop_or_error() {
    let store = InMemoryStore::new();
    // Appending zero events should either be rejected or be a safe no-op
    let result = store.append(&TestId("s1"), None, &[]).await;
    // This should succeed (no-op) but version should not change
    assert!(result.is_ok());

    // Read should return empty stream
    let mut stream = store
        .read_stream(&TestId("s1"), Version::INITIAL)
        .await
        .unwrap();
    assert!(
        stream.next().await.is_none(),
        "Empty append should not create any events"
    );
}

// NOTE: H2 (upcaster empty event_type) and H3 (upcaster version downgrade)
// tests have been removed. The replacement SchemaTransform trait uses
// declarative source_version/to_version — the pipeline validates these
// at registration time, not runtime. See: schema_transform_tests.rs,
// transform_chain_new_tests.rs, pipeline_tests.rs

// =============================================================================
// H5: Append doesn't validate version sequence of envelopes
// =============================================================================

#[tokio::test]
async fn h5_append_with_non_sequential_versions() {
    let store = InMemoryStore::new();

    // Envelopes with non-sequential versions: v3, v1, v5
    let envelopes = vec![
        pending_envelope(Version::new(3).unwrap())
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope(Version::new(1).unwrap())
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope(Version::new(5).unwrap())
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
    ];

    // This should fail — versions must be sequential
    let result = store.append(&TestId("s1"), None, &envelopes).await;
    // Currently the test adapter accepts this — it should NOT
    // The EventStore facade (when built) should validate this
    assert!(
        result.is_err(),
        "Append should reject non-sequential version order"
    );
}

// =============================================================================
// H5 variant: Append with duplicate versions
// =============================================================================

#[tokio::test]
async fn h5_append_with_duplicate_versions() {
    let store = InMemoryStore::new();

    let envelopes = vec![
        pending_envelope(Version::INITIAL)
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope(Version::INITIAL)
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(), // dup!
    ];

    let result = store.append(&TestId("s1"), None, &envelopes).await;
    assert!(result.is_err(), "Append should reject duplicate versions");
}

// =============================================================================
// H4: StoreError has heap-allocated Box<dyn Error>
// =============================================================================

#[test]
fn h4_store_error_size() {
    // Document the size of StoreError for IoT awareness.
    // Box<dyn Error> means heap allocation on error paths.
    let size = std::mem::size_of::<StoreError>();
    // This should be reasonable — if it's too large, consider boxing the whole error
    assert!(
        size <= 128,
        "StoreError is {size} bytes — too large for stack on constrained devices"
    );
}

// =============================================================================
// M1/M2: Clone and PartialEq on envelopes
// =============================================================================

#[test]
fn m2_pending_envelope_no_partial_eq() {
    // Currently PendingEnvelope doesn't implement PartialEq.
    // This test documents the gap — can't easily compare envelopes in tests.
    let e1 = pending_envelope(Version::INITIAL)
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    let e2 = pending_envelope(Version::INITIAL)
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    // Can't do assert_eq!(e1, e2) — no PartialEq
    // So we compare field by field
    assert_eq!(e1.version(), e2.version());
    assert_eq!(e1.event_type(), e2.event_type());
    assert_eq!(e1.payload(), e2.payload());
}

// =============================================================================
// Read nonexistent stream
// =============================================================================

#[tokio::test]
async fn read_nonexistent_stream_returns_empty() {
    let store = InMemoryStore::new();
    let mut stream = store
        .read_stream(&TestId("does-not-exist"), Version::INITIAL)
        .await
        .unwrap();
    assert!(
        stream.next().await.is_none(),
        "Reading a nonexistent stream should return empty, not error"
    );
}

// =============================================================================
// Multiple streams isolation
// =============================================================================

#[tokio::test]
async fn streams_are_isolated() {
    let store = InMemoryStore::new();

    let e1 = vec![
        pending_envelope(Version::INITIAL)
            .event_type("EventA")
            .payload(vec![1])
            .build_without_metadata(),
    ];
    let e2 = vec![
        pending_envelope(Version::INITIAL)
            .event_type("EventB")
            .payload(vec![2])
            .build_without_metadata(),
    ];

    store.append(&TestId("stream-a"), None, &e1).await.unwrap();
    store.append(&TestId("stream-b"), None, &e2).await.unwrap();

    // Read stream-a — should only see EventA
    let mut stream = store
        .read_stream(&TestId("stream-a"), Version::INITIAL)
        .await
        .unwrap();
    let envelope = stream.next().await.unwrap().unwrap();
    assert_eq!(envelope.event_type(), "EventA");
    assert_eq!(envelope.payload(), &[1]);
    drop(envelope);
    assert!(stream.next().await.is_none());

    // Read stream-b — should only see EventB
    let mut stream = store
        .read_stream(&TestId("stream-b"), Version::INITIAL)
        .await
        .unwrap();
    let envelope = stream.next().await.unwrap().unwrap();
    assert_eq!(envelope.event_type(), "EventB");
    assert_eq!(envelope.payload(), &[2]);
    drop(envelope);
    assert!(stream.next().await.is_none());
}

// =============================================================================
// L: StoreError variant exhaustiveness
// =============================================================================

#[test]
fn store_error_variants_are_known() {
    let err = StoreError::StreamNotFound {
        stream_id: "test".to_stream_label(),
    };
    match err {
        StoreError::Conflict { .. } => {}
        StoreError::StreamNotFound { .. } => {}
        StoreError::Codec(_) => {}
        StoreError::Adapter(_) => {}
        StoreError::Kernel(_) => {} // If you add a variant, add it here
    }
}
