use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;

#[test]
fn pending_envelope_accessors() {
    let envelope = pending_envelope("user-123".into())
        .version(Version::from_persisted(1))
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .build_without_metadata();

    assert_eq!(envelope.stream_id(), "user-123");
    assert_eq!(envelope.version(), Version::from_persisted(1));
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
    let envelope = pending_envelope("order-1".into())
        .version(Version::from_persisted(1))
        .event_type("OrderPlaced")
        .payload(vec![4, 5, 6])
        .build(meta.clone());

    assert_eq!(envelope.metadata(), &meta);
}

#[test]
fn pending_envelope_unit_metadata_default() {
    let envelope = pending_envelope("stream".into())
        .version(Version::from_persisted(1))
        .event_type("Event")
        .payload(vec![])
        .build_without_metadata();
    assert_eq!(envelope.metadata(), &());
}

#[test]
fn persisted_envelope_borrows_from_source() {
    let stream_id = String::from("user-456");
    let event_type = String::from("UserActivated");
    let payload = vec![10, 20, 30];

    let envelope = PersistedEnvelope::<()>::new(
        &stream_id,
        Version::from_persisted(3),
        &event_type,
        &payload,
        (),
    );

    assert_eq!(envelope.stream_id(), "user-456");
    assert_eq!(envelope.version(), Version::from_persisted(3));
    assert_eq!(envelope.event_type(), "UserActivated");
    assert_eq!(envelope.payload(), &[10, 20, 30]);
}

#[test]
fn persisted_envelope_metadata_is_owned() {
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        tenant: String,
    }

    let stream_id = "s";
    let event_type = "E";
    let payload = [1u8];

    let envelope = PersistedEnvelope::new(
        stream_id,
        Version::from_persisted(1),
        event_type,
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
    let source_stream = "my-stream";
    let source_type = "MyEvent";
    let source_payload = [1u8, 2, 3, 4, 5];

    let envelope = PersistedEnvelope::<()>::new(
        source_stream,
        Version::from_persisted(1),
        source_type,
        &source_payload,
        (),
    );

    // Verify the envelope borrows from the source —
    // the pointers should point into the same memory
    assert!(std::ptr::eq(
        envelope.stream_id().as_bytes().as_ptr(),
        source_stream.as_ptr()
    ));
    assert!(std::ptr::eq(
        envelope.payload().as_ptr(),
        source_payload.as_ptr()
    ));
}

#[test]
fn pending_envelope_debug_output() {
    let envelope = pending_envelope("user-abc".into())
        .version(Version::from_persisted(7))
        .event_type("UserCreated")
        .payload(vec![1, 2, 3])
        .build_without_metadata();
    let debug = format!("{envelope:?}");
    assert!(
        debug.contains("PendingEnvelope"),
        "Debug should contain type name"
    );
    assert!(
        debug.contains("user-abc"),
        "Debug should contain stream_id value"
    );
}

#[test]
fn persisted_envelope_debug_output() {
    let stream_id = "order-99";
    let event_type = "OrderPlaced";
    let payload = [10u8, 20];
    let envelope = PersistedEnvelope::<()>::new(
        stream_id,
        Version::from_persisted(2),
        event_type,
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
    let without = pending_envelope("stream-1".into())
        .version(Version::from_persisted(5))
        .event_type("Evt")
        .payload(vec![9, 8, 7])
        .build_without_metadata();

    let with_unit = pending_envelope("stream-1".into())
        .version(Version::from_persisted(5))
        .event_type("Evt")
        .payload(vec![9, 8, 7])
        .build(());

    assert_eq!(without.stream_id(), with_unit.stream_id());
    assert_eq!(without.version(), with_unit.version());
    assert_eq!(without.event_type(), with_unit.event_type());
    assert_eq!(without.payload(), with_unit.payload());
    assert_eq!(without.metadata(), with_unit.metadata());
}
