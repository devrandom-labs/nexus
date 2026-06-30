#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use bytes::Bytes;
use nexus::Version;
use nexus_store::envelope::{EnvelopeError, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::value::SchemaVersion;

fn make_persisted(
    version: Version,
    event_type: &str,
    schema_version: u32,
    payload: &[u8],
) -> PersistedEnvelope {
    let mut buf = Vec::with_capacity(event_type.len() + payload.len());
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(payload);
    let value = Bytes::from(buf);
    let et_end = u32::try_from(event_type.len()).expect("fits");
    let pl_end = u32::try_from(event_type.len() + payload.len()).expect("fits");
    let sv = SchemaVersion::from_u32(schema_version).expect("nonzero test fixture");
    PersistedEnvelope::try_new(version, value, sv, 0..et_end, et_end..pl_end, None)
        .expect("valid test fixture")
}

#[test]
fn pending_envelope_accessors() {
    let envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .expect("valid payload")
        .build();

    assert_eq!(envelope.version(), Version::new(1).unwrap());
    assert_eq!(envelope.event_type(), "UserCreated");
    assert_eq!(envelope.payload(), &[1, 2, 3]);
    assert!(envelope.metadata().is_none());
}

#[test]
fn pending_envelope_with_metadata() {
    let meta = b"corr-1".to_vec();
    let envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("OrderPlaced")
        .payload(vec![4, 5, 6])
        .expect("valid payload")
        .with_metadata(meta.clone())
        .expect("valid metadata");

    assert_eq!(envelope.metadata(), Some(meta.as_slice()));
}

#[test]
fn persisted_envelope_accessors() {
    let payload = [10u8, 20, 30];
    let envelope = make_persisted(Version::new(3).unwrap(), "UserActivated", 1, &payload);

    assert_eq!(envelope.version(), Version::new(3).unwrap());
    assert_eq!(envelope.event_type(), "UserActivated");
    assert_eq!(envelope.payload(), &[10, 20, 30]);
    assert!(envelope.metadata().is_none());
}

#[test]
fn persisted_envelope_metadata_bytes() {
    let event_type = "E";
    let payload = [1u8];
    let meta = b"tenant:acme".to_vec();

    let mut buf = Vec::with_capacity(event_type.len() + payload.len() + meta.len());
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(&payload);
    buf.extend_from_slice(&meta);
    let value = Bytes::from(buf);
    let et_end = 1u32;
    let pl_end = 2u32;
    let meta_end = 2u32 + u32::try_from(meta.len()).unwrap();
    let envelope = PersistedEnvelope::try_new(
        Version::new(1).unwrap(),
        value,
        SchemaVersion::INITIAL,
        0..et_end,
        et_end..pl_end,
        Some(pl_end..meta_end),
    )
    .expect("valid envelope");

    assert_eq!(envelope.metadata(), Some(b"tenant:acme".as_slice()));
}

#[test]
fn persisted_envelope_payload_is_correct() {
    let source_payload = [1u8, 2, 3, 4, 5];
    let envelope = make_persisted(Version::new(1).unwrap(), "MyEvent", 1, &source_payload);
    assert_eq!(envelope.payload(), &source_payload);
}

#[test]
fn pending_envelope_debug_output() {
    let envelope = pending_envelope(Version::new(7).unwrap())
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .expect("valid payload")
        .build();
    let debug = format!("{envelope:?}");
    assert!(
        debug.contains("PendingEnvelope"),
        "Debug should contain type name"
    );
}

#[test]
fn persisted_envelope_debug_output() {
    let envelope = make_persisted(Version::new(2).unwrap(), "OrderPlaced", 1, &[10u8, 20]);
    let debug = format!("{envelope:?}");
    assert!(
        debug.contains("PersistedEnvelope"),
        "Debug should contain type name"
    );
}

#[test]
fn build_without_metadata_has_no_metadata() {
    let env = pending_envelope(Version::new(5).unwrap())
        .event_type("Evt")
        .payload(vec![9, 8, 7])
        .expect("valid payload")
        .build();

    assert!(env.metadata().is_none());
    assert_eq!(env.version(), Version::new(5).unwrap());
    assert_eq!(env.event_type(), "Evt");
    assert_eq!(env.payload(), &[9, 8, 7]);
}

// =============================================================================
// PersistedEnvelope::try_new() tests
// =============================================================================

// Note: `try_new` no longer takes a raw `u32` schema_version — the `SchemaVersion`
// newtype makes zero structurally unrepresentable. The corrupt-on-disk zero case
// is now surfaced one layer down by `wire::decode_frame`, pinned in
// `wire::tests::decode_frame_rejects_corrupt_schema_version_zero`.

#[test]
fn try_new_accepts_valid_schema_version() {
    let result = make_persisted(Version::new(1).unwrap(), "E", 1, &[]);
    assert_eq!(result.schema_version(), 1);
}

#[test]
fn try_new_accepts_max_schema_version() {
    let result = make_persisted(Version::new(1).unwrap(), "E", u32::MAX, &[]);
    assert_eq!(result.schema_version(), u32::MAX);
}

#[test]
fn try_new_rejects_out_of_bounds_range() {
    // event_type range goes past value length
    let result = PersistedEnvelope::try_new(
        Version::new(1).unwrap(),
        Bytes::from_static(b"E"),
        SchemaVersion::INITIAL,
        0..100, // ATTACK: range beyond buffer
        100..100,
        None,
    );
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        EnvelopeError::RangeOutOfBounds { .. }
    ));
}

#[test]
fn try_new_rejects_invalid_utf8_event_type() {
    // 0xFF 0xFE is invalid UTF-8
    let value = Bytes::from(vec![0xFF, 0xFE, b'x']);
    let result = PersistedEnvelope::try_new(
        Version::new(1).unwrap(),
        value,
        SchemaVersion::INITIAL,
        0..2, // invalid UTF-8 bytes as event_type
        2..3,
        None,
    );
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        EnvelopeError::InvalidUtf8 { .. }
    ));
}
