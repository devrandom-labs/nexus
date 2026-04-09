#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;

#[test]
fn pending_envelope_accessors() {
    let envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .build_without_metadata();

    assert_eq!(envelope.version(), Version::new(1).unwrap());
    assert_eq!(envelope.event_type(), "UserCreated");
    assert_eq!(envelope.payload(), &[1, 2, 3]);
}

#[test]
fn pending_envelope_with_metadata() {
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        correlation_id: String,
    }

    let meta = Meta {
        correlation_id: "corr-1".into(),
    };
    let envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("OrderPlaced")
        .payload(vec![4, 5, 6])
        .build(meta.clone());

    assert_eq!(envelope.metadata(), &meta);
}

#[test]
fn persisted_envelope_borrows_from_source() {
    let event_type = String::from("UserActivated");
    let payload = vec![10, 20, 30];

    let envelope = PersistedEnvelope::<()>::new_unchecked(
        Version::new(3).unwrap(),
        &event_type,
        1,
        &payload,
        (),
    );

    assert_eq!(envelope.version(), Version::new(3).unwrap());
    assert_eq!(envelope.event_type(), "UserActivated");
    assert_eq!(envelope.payload(), &[10, 20, 30]);
}

#[test]
fn persisted_envelope_metadata_is_owned() {
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        tenant: String,
    }

    let event_type = "E";
    let payload = [1u8];

    let envelope = PersistedEnvelope::new_unchecked(
        Version::new(1).unwrap(),
        event_type,
        1,
        &payload,
        Meta {
            tenant: "acme".into(),
        },
    );

    assert_eq!(
        envelope.metadata(),
        &Meta {
            tenant: "acme".into()
        }
    );
}

#[test]
fn persisted_envelope_zero_allocation_for_core_fields() {
    let source_type = "MyEvent";
    let source_payload = [1u8, 2, 3, 4, 5];

    let envelope = PersistedEnvelope::<()>::new_unchecked(
        Version::new(1).unwrap(),
        source_type,
        1,
        &source_payload,
        (),
    );

    // Verify the envelope borrows from the source —
    // the pointers should point into the same memory
    assert!(std::ptr::eq(
        envelope.payload().as_ptr(),
        source_payload.as_ptr()
    ));
}

#[test]
fn pending_envelope_debug_output() {
    let envelope = pending_envelope(Version::new(7).unwrap())
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .build_without_metadata();
    let debug = format!("{envelope:?}");
    assert!(
        debug.contains("PendingEnvelope"),
        "Debug should contain type name"
    );
}

#[test]
fn persisted_envelope_debug_output() {
    let event_type = "OrderPlaced";
    let payload = [10u8, 20];
    let envelope = PersistedEnvelope::<()>::new_unchecked(
        Version::new(2).unwrap(),
        event_type,
        1,
        &payload,
        (),
    );
    let debug = format!("{envelope:?}");
    assert!(
        debug.contains("PersistedEnvelope"),
        "Debug should contain type name"
    );
}

#[test]
fn build_without_metadata_equals_build_unit() {
    let without = pending_envelope(Version::new(5).unwrap())
        .event_type("Evt")
        .payload(vec![9, 8, 7])
        .build_without_metadata();

    let with_unit = pending_envelope(Version::new(5).unwrap())
        .event_type("Evt")
        .payload(vec![9, 8, 7])
        .build(());

    assert_eq!(without.version(), with_unit.version());
    assert_eq!(without.event_type(), with_unit.event_type());
    assert_eq!(without.payload(), with_unit.payload());
    assert_eq!(without.metadata(), with_unit.metadata());
}

// =============================================================================
// PersistedEnvelope::try_new() tests
// =============================================================================

#[test]
fn try_new_rejects_zero_schema_version() {
    let result = PersistedEnvelope::<()>::try_new(Version::new(1).unwrap(), "E", 0, &[], ());
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("schema version"), "should describe the error");
}

#[test]
fn try_new_accepts_valid_schema_version() {
    let result = PersistedEnvelope::<()>::try_new(Version::new(1).unwrap(), "E", 1, &[], ());
    assert!(result.is_ok());
    let env = result.unwrap();
    assert_eq!(env.schema_version(), 1);
}

#[test]
fn try_new_accepts_max_schema_version() {
    let result = PersistedEnvelope::<()>::try_new(Version::new(1).unwrap(), "E", u32::MAX, &[], ());
    assert!(result.is_ok());
    assert_eq!(result.unwrap().schema_version(), u32::MAX);
}
