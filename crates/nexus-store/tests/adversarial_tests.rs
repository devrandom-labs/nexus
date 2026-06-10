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

use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;
use nexus_store::store::GlobalSeq;

fn build_persisted(
    version: Version,
    global_seq: GlobalSeq,
    event_type: &str,
    payload: &[u8],
) -> PersistedEnvelope {
    let mut buf = Vec::with_capacity(event_type.len() + payload.len());
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(payload);
    let value = bytes::Bytes::from(buf);
    let et_end = u32::try_from(event_type.len()).expect("event_type fits u32");
    let pl_end = u32::try_from(event_type.len() + payload.len()).expect("payload fits u32");
    PersistedEnvelope::try_new(
        version,
        global_seq,
        value,
        nexus_store::value::SchemaVersion::INITIAL,
        0..et_end,
        et_end..pl_end,
        None,
    )
    .expect("test fixture envelope")
}

// =============================================================================
// 1. Maximum version envelope
// =============================================================================

#[test]
fn max_version_envelope() {
    let envelope = pending_envelope(Version::new(u64::MAX).unwrap())
        .event_type("Event")
        .payload(vec![])
        .expect("valid payload")
        .build();

    assert_eq!(envelope.version().as_u64(), u64::MAX);

    // Also verify via PersistedEnvelope
    let persisted = build_persisted(
        Version::new(u64::MAX).unwrap(),
        GlobalSeq::INITIAL,
        "Event",
        &[],
    );
    assert_eq!(persisted.version().as_u64(), u64::MAX);
}

// =============================================================================
// 2. Empty payload
// =============================================================================

#[test]
fn empty_payload() {
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("EmptyEvent")
        .payload(vec![])
        .expect("valid payload")
        .build();

    assert!(envelope.payload().is_empty());
    assert_eq!(envelope.payload().len(), 0);
}

// =============================================================================
// 3. Large payload (1 MB)
// =============================================================================

#[test]
fn large_payload() {
    let size = 1_000_000;
    let payload = vec![0xFF_u8; size];
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("LargeEvent")
        .payload(payload)
        .expect("valid payload")
        .build();

    assert_eq!(envelope.payload().len(), size);
    assert!(
        envelope.payload().iter().all(|&b| b == 0xFF),
        "every byte must be 0xFF"
    );
}

// =============================================================================
// 4. Persisted envelope with all 256 byte values
// =============================================================================

#[test]
fn persisted_envelope_binary_payload() {
    let payload: Vec<u8> = (0..=255).collect();
    assert_eq!(payload.len(), 256);

    let persisted = build_persisted(
        Version::INITIAL,
        GlobalSeq::INITIAL,
        "BinaryEvent",
        &payload,
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
