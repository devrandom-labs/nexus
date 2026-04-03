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

use nexus::Version;
use nexus_store::error::StoreError;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;
use nexus_store::upcaster::EventUpcaster;

// =============================================================================
// C2: Builder accepts any String — content validation is the facade's job
// =============================================================================

#[test]
fn c2_builder_accepts_empty_stream_id() {
    // The typestate builder ensures all fields are SET but cannot validate
    // string CONTENT at compile time. Empty-string validation is deferred
    // to the EventStore facade.
    let envelope = pending_envelope(String::new())
        .version(Version::from_persisted(1))
        .event_type("Event")
        .payload(vec![1])
        .build_without_metadata();
    assert_eq!(envelope.stream_id(), "");
}

#[test]
fn c2_builder_accepts_empty_event_type() {
    // Same rationale: the builder takes &'static str, so "" is valid.
    // The EventStore facade must reject empty event_type at runtime.
    let envelope = pending_envelope("stream".into())
        .version(Version::from_persisted(1))
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
    let result = store.append("s1", Version::INITIAL, &[]).await;
    // This should succeed (no-op) but version should not change
    assert!(result.is_ok());

    // Read should return empty stream
    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
    assert!(
        stream.next().await.is_none(),
        "Empty append should not create any events"
    );
}

// =============================================================================
// H2: EventUpcaster can return empty event_type
// =============================================================================

#[test]
fn h2_upcaster_can_return_empty_event_type() {
    // DOCUMENTED RISK: The EventUpcaster trait cannot prevent implementations
    // from returning empty event_type. The EventStore facade MUST validate
    // upcaster output before passing to the codec.
    struct BadUpcaster;
    impl EventUpcaster for BadUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true
        }
        fn upcast(&self, _: &str, _: u32, payload: &[u8]) -> (String, u32, Vec<u8>) {
            (String::new(), 2, payload.to_vec())
        }
    }

    let upcaster = BadUpcaster;
    let (event_type, _, _) = upcaster.upcast("OldEvent", 1, b"data");
    // This IS empty — the facade must catch it (tested when facade is built)
    assert!(
        event_type.is_empty(),
        "BadUpcaster returns empty — facade must validate"
    );
}

// =============================================================================
// H3: EventUpcaster can downgrade schema_version
// =============================================================================

#[test]
fn h3_upcaster_can_downgrade_version() {
    // DOCUMENTED RISK: The EventUpcaster trait cannot prevent implementations
    // from downgrading schema_version. The EventStore facade MUST validate
    // that upcasted version > input version.
    struct DowngradeUpcaster;
    impl EventUpcaster for DowngradeUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true
        }
        fn upcast(&self, _: &str, _: u32, payload: &[u8]) -> (String, u32, Vec<u8>) {
            ("Event".into(), 0, payload.to_vec())
        }
    }

    let upcaster = DowngradeUpcaster;
    let (_, new_version, _) = upcaster.upcast("Event", 1, b"data");
    // Version went DOWN — facade must catch it (tested when facade is built)
    assert_eq!(
        new_version, 0,
        "DowngradeUpcaster returns 0 — facade must validate"
    );
}

// =============================================================================
// H5: Append doesn't validate version sequence of envelopes
// =============================================================================

#[tokio::test]
async fn h5_append_with_non_sequential_versions() {
    let store = InMemoryStore::new();

    // Envelopes with non-sequential versions: v3, v1, v5
    let envelopes = vec![
        pending_envelope("s1".into())
            .version(Version::from_persisted(3))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(5))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
    ];

    // This should fail — versions must be sequential
    let result = store.append("s1", Version::INITIAL, &envelopes).await;
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
        pending_envelope("s1".into())
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![])
            .build_without_metadata(), // dup!
    ];

    let result = store.append("s1", Version::INITIAL, &envelopes).await;
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
    let e1 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    let e2 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    // Can't do assert_eq!(e1, e2) — no PartialEq
    // So we compare field by field
    assert_eq!(e1.stream_id(), e2.stream_id());
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
        .read_stream("does-not-exist", Version::INITIAL)
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
        pending_envelope("stream-a".into())
            .version(Version::from_persisted(1))
            .event_type("EventA")
            .payload(vec![1])
            .build_without_metadata(),
    ];
    let e2 = vec![
        pending_envelope("stream-b".into())
            .version(Version::from_persisted(1))
            .event_type("EventB")
            .payload(vec![2])
            .build_without_metadata(),
    ];

    store
        .append("stream-a", Version::INITIAL, &e1)
        .await
        .unwrap();
    store
        .append("stream-b", Version::INITIAL, &e2)
        .await
        .unwrap();

    // Read stream-a — should only see EventA
    let mut stream = store
        .read_stream("stream-a", Version::INITIAL)
        .await
        .unwrap();
    let envelope = stream.next().await.unwrap().unwrap();
    assert_eq!(envelope.event_type(), "EventA");
    assert_eq!(envelope.payload(), &[1]);
    drop(envelope);
    assert!(stream.next().await.is_none());

    // Read stream-b — should only see EventB
    let mut stream = store
        .read_stream("stream-b", Version::INITIAL)
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
        stream_id: nexus::ErrorId::from_display(&"test"),
    };
    match err {
        StoreError::Conflict { .. } => {}
        StoreError::StreamNotFound { .. } => {}
        StoreError::Codec(_) => {}
        StoreError::Adapter(_) => {} // If you add a variant, add it here
    }
}
