//! Adversarial input tests for nexus-store.
//!
//! Stresses the builder and envelope types with edge-case inputs:
//! Unicode, large payloads, path traversal, exotic metadata, etc.

#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for clarity and brevity"
)]
#![allow(
    clippy::expect_used,
    reason = "tests use expect for clarity and brevity"
)]
#![allow(clippy::str_to_string, reason = "tests use to_string/to_owned freely")]
#![allow(
    clippy::shadow_reuse,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::shadow_unrelated,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::as_conversions,
    reason = "tests use as casts for index-to-byte conversions"
)]

use std::collections::HashMap;

use nexus::StreamId;
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;

#[allow(clippy::unwrap_used, reason = "test helper")]
fn sid(s: &str) -> StreamId {
    StreamId::from_persisted(s).unwrap()
}

// =============================================================================
// 1. Unicode stream ID with emoji
// =============================================================================

#[test]
fn unicode_stream_id() {
    let envelope = pending_envelope(sid("user-\u{1F600}-stream"))
        .version(Version::from_persisted(1))
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .build_without_metadata();

    assert_eq!(envelope.stream_id().as_str(), "user-\u{1F600}-stream");

    // Round-trip through PersistedEnvelope
    let owned_str = envelope.stream_id().as_str().to_owned();
    let persisted = PersistedEnvelope::<()>::new(
        &owned_str,
        envelope.version(),
        envelope.event_type(),
        1,
        envelope.payload(),
        (),
    );
    assert_eq!(persisted.stream_id(), "user-\u{1F600}-stream");
}

// =============================================================================
// 2. RTL and zero-width joiner characters in stream ID
// =============================================================================

#[test]
fn rtl_and_zero_width_joiner_stream_id() {
    // Arabic text + ZWJ (U+200D) + more Arabic
    let raw = "\u{0639}\u{0631}\u{0628}\u{064A}\u{200D}\u{0645}\u{0632}\u{064A}\u{062F}";
    let stream_id = sid(raw);
    let envelope = pending_envelope(stream_id)
        .version(Version::from_persisted(1))
        .event_type("Event")
        .payload(vec![])
        .build_without_metadata();

    // Must be stored literally — no normalization or stripping
    assert_eq!(envelope.stream_id().as_str(), raw);
    assert!(
        envelope.stream_id().as_str().contains('\u{200D}'),
        "ZWJ character must not be stripped"
    );
    assert_eq!(envelope.stream_id().as_str().len(), raw.len());
}

// =============================================================================
// 3. Maximum version envelope
// =============================================================================

#[test]
fn max_version_envelope() {
    let envelope = pending_envelope(sid("stream"))
        .version(Version::from_persisted(u64::MAX))
        .event_type("Event")
        .payload(vec![])
        .build_without_metadata();

    assert_eq!(envelope.version().as_u64(), u64::MAX);

    // Also verify via PersistedEnvelope
    let persisted = PersistedEnvelope::<()>::new(
        "stream",
        Version::from_persisted(u64::MAX),
        "Event",
        1,
        &[],
        (),
    );
    assert_eq!(persisted.version().as_u64(), u64::MAX);
}

// =============================================================================
// 4. Empty payload
// =============================================================================

#[test]
fn empty_payload() {
    let envelope = pending_envelope(sid("stream"))
        .version(Version::from_persisted(1))
        .event_type("EmptyEvent")
        .payload(vec![])
        .build_without_metadata();

    assert!(envelope.payload().is_empty());
    assert_eq!(envelope.payload().len(), 0);
}

// =============================================================================
// 5. Large payload (1 MB)
// =============================================================================

#[test]
fn large_payload() {
    let size = 1_000_000;
    let payload = vec![0xFF_u8; size];
    let envelope = pending_envelope(sid("stream"))
        .version(Version::from_persisted(1))
        .event_type("LargeEvent")
        .payload(payload)
        .build_without_metadata();

    assert_eq!(envelope.payload().len(), size);
    assert!(
        envelope.payload().iter().all(|&b| b == 0xFF),
        "every byte must be 0xFF"
    );
}

// =============================================================================
// 6. Path traversal in stream ID
// =============================================================================

#[test]
fn path_traversal_stream_id() {
    let raw = "../../../etc/passwd";
    let stream_id = sid(raw);
    let envelope = pending_envelope(stream_id)
        .version(Version::from_persisted(1))
        .event_type("Event")
        .payload(vec![42])
        .build_without_metadata();

    // Must be stored literally — no path interpretation
    assert_eq!(envelope.stream_id().as_str(), raw);
    assert!(envelope.stream_id().as_str().starts_with("../"));
}

// =============================================================================
// 7. Null bytes in stream ID
// =============================================================================

#[test]
fn null_bytes_in_stream_id_rejected_by_stream_id() {
    // StreamId rejects null bytes at construction time — this is the
    // desired compile-time safety improvement from the StreamId type.
    let result = StreamId::from_persisted("stream\0injected");
    assert!(result.is_err(), "StreamId should reject null bytes");
}

// =============================================================================
// 8. Exotic metadata with deeply nested generics
// =============================================================================

#[test]
fn exotic_metadata_nested_generics() {
    let mut inner_map: HashMap<String, Vec<u8>> = HashMap::new();
    inner_map.insert("key1".to_owned(), vec![1, 2, 3]);
    inner_map.insert("key2".to_owned(), vec![]);

    let metadata: Vec<Option<HashMap<String, Vec<u8>>>> =
        vec![Some(inner_map), None, Some(HashMap::new())];

    let envelope = pending_envelope(sid("stream"))
        .version(Version::from_persisted(1))
        .event_type("ExoticEvent")
        .payload(vec![0xAB])
        .build(metadata);

    // Verify the nested structure survived
    assert_eq!(envelope.metadata().len(), 3);
    assert!(envelope.metadata()[0].is_some());
    assert!(envelope.metadata()[1].is_none());
    assert!(envelope.metadata()[2].is_some());

    let first = envelope.metadata()[0].as_ref().unwrap();
    assert_eq!(first.get("key1").unwrap(), &vec![1_u8, 2, 3]);
    assert_eq!(first.get("key2").unwrap(), &vec![] as &Vec<u8>);
}

// =============================================================================
// 9. Zero-sized metadata
// =============================================================================

#[test]
fn zero_sized_metadata() {
    #[derive(Debug, Clone, PartialEq)]
    struct Zst;

    let envelope = pending_envelope(sid("stream"))
        .version(Version::from_persisted(1))
        .event_type("ZstEvent")
        .payload(vec![])
        .build(Zst);

    assert_eq!(std::mem::size_of_val(envelope.metadata()), 0);
    assert_eq!(envelope.metadata(), &Zst);
}

// =============================================================================
// 10. Persisted envelope with all 256 byte values
// =============================================================================

#[test]
fn persisted_envelope_binary_payload() {
    let payload: Vec<u8> = (0..=255).collect();
    assert_eq!(payload.len(), 256);

    let persisted = PersistedEnvelope::<()>::new(
        "binary-stream",
        Version::from_persisted(1),
        "BinaryEvent",
        1,
        &payload,
        (),
    );

    assert_eq!(persisted.payload().len(), 256);
    for (i, &byte) in persisted.payload().iter().enumerate() {
        assert_eq!(
            byte,
            u8::try_from(i).unwrap(),
            "byte at index {i} should be {i}"
        );
    }
}
