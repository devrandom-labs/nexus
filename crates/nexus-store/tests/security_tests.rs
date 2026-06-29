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

use futures::StreamExt;
use nexus::{ErrorId, Version};
use nexus_store::InMemoryStoreError;
use nexus_store::error::StoreError;
use nexus_store::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;

/// Concrete `StoreError` for tests using `InMemoryStore` with no codec/upcaster.
type TestStoreError = StoreError<InMemoryStoreError, std::io::Error, std::io::Error>;

fn label(s: &str) -> ErrorId {
    ErrorId::from_display(&s)
}

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
        .expect("valid payload")
        .build();
    assert_eq!(envelope.event_type(), "");
}

// =============================================================================
// H1: RawEventStore::append accepts empty envelope slice
// =============================================================================

#[tokio::test]
async fn h1_append_empty_envelopes_is_noop_or_error() {
    let store = InMemoryStore::new();
    // Appending zero events should either be rejected or be a safe no-op
    let result = store
        .append(
            &nexus_store::StreamKey::from_slice("s1".as_bytes()),
            None,
            &[],
        )
        .await;
    // This should succeed (no-op) but version should not change
    assert!(result.is_ok());

    // Read should return empty stream
    let mut stream = store
        .read_stream(
            &nexus_store::StreamKey::from_slice("s1".as_bytes()),
            Version::INITIAL,
        )
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
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(1).unwrap())
            .event_type("E")
            .payload(vec![])
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(5).unwrap())
            .event_type("E")
            .payload(vec![])
            .expect("valid payload")
            .build(),
    ];

    // This should fail — versions must be sequential
    let result = store
        .append(
            &nexus_store::StreamKey::from_slice("s1".as_bytes()),
            None,
            &envelopes,
        )
        .await;
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
            .expect("valid payload")
            .build(),
        pending_envelope(Version::INITIAL)
            .event_type("E")
            .payload(vec![])
            .expect("valid payload")
            .build(), // dup!
    ];

    let result = store
        .append(
            &nexus_store::StreamKey::from_slice("s1".as_bytes()),
            None,
            &envelopes,
        )
        .await;
    assert!(result.is_err(), "Append should reject duplicate versions");
}

// =============================================================================
// H4: StoreError has concrete typed errors — no heap allocation
// =============================================================================

#[test]
fn h4_store_error_size() {
    // Document the size of StoreError for IoT awareness.
    // With concrete types, no Box<dyn Error> heap allocation on error paths.
    let size = std::mem::size_of::<TestStoreError>();
    // This should be reasonable — if it's too large, consider boxing the whole error
    assert!(
        size <= 256,
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
        .expect("valid payload")
        .build();
    let e2 = pending_envelope(Version::INITIAL)
        .event_type("E")
        .payload(vec![1])
        .expect("valid payload")
        .build();
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
        .read_stream(
            &nexus_store::StreamKey::from_slice("does-not-exist".as_bytes()),
            Version::INITIAL,
        )
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
            .expect("valid payload")
            .build(),
    ];
    let e2 = vec![
        pending_envelope(Version::INITIAL)
            .event_type("EventB")
            .payload(vec![2])
            .expect("valid payload")
            .build(),
    ];

    store
        .append(
            &nexus_store::StreamKey::from_slice("stream-a".as_bytes()),
            None,
            &e1,
        )
        .await
        .unwrap();
    store
        .append(
            &nexus_store::StreamKey::from_slice("stream-b".as_bytes()),
            None,
            &e2,
        )
        .await
        .unwrap();

    // Read stream-a — should only see EventA
    let mut stream = store
        .read_stream(
            &nexus_store::StreamKey::from_slice("stream-a".as_bytes()),
            Version::INITIAL,
        )
        .await
        .unwrap();
    let envelope = stream.next().await.unwrap().unwrap();
    assert_eq!(envelope.event_type(), "EventA");
    assert_eq!(envelope.payload(), &[1]);
    drop(envelope);
    assert!(stream.next().await.is_none());

    // Read stream-b — should only see EventB
    let mut stream = store
        .read_stream(
            &nexus_store::StreamKey::from_slice("stream-b".as_bytes()),
            Version::INITIAL,
        )
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
    let err: TestStoreError = StoreError::StreamNotFound {
        stream_id: label("test"),
    };
    // StoreError is #[non_exhaustive] at the 1.0 freeze (#209), so a separate
    // test crate cannot match it exhaustively — the `_` arm covers the open
    // tail. This trades the old compile-time exhaustiveness guard for the
    // non_exhaustive contract (new error variants are a minor, not major, bump).
    match err {
        StoreError::Conflict { .. } => {}
        StoreError::StreamNotFound { .. } => {}
        StoreError::Encode(_) => {}
        StoreError::Decode(_) => {}
        StoreError::Adapter(_) => {}
        StoreError::Kernel(_) => {}
        StoreError::VersionOverflow => {}
        StoreError::EnvelopeSynthesis(_) => {}
        StoreError::Envelope(_) => {}
        _ => {}
    }
}
