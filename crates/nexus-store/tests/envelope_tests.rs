use nexus::Version;
use nexus_store::envelope::PendingEnvelope;

#[test]
fn pending_envelope_accessors() {
    let envelope = PendingEnvelope::<()>::new(
        "user-123".to_string(),
        Version::from_persisted(1),
        "UserCreated",
        vec![1, 2, 3],
        (),
    );

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
    let envelope = PendingEnvelope::new(
        "order-1".to_string(),
        Version::from_persisted(1),
        "OrderPlaced",
        vec![4, 5, 6],
        meta.clone(),
    );

    assert_eq!(envelope.metadata(), &meta);
}

#[test]
fn pending_envelope_unit_metadata_default() {
    let envelope = PendingEnvelope::new(
        "stream".to_string(),
        Version::from_persisted(1),
        "Event",
        vec![],
        (),
    );
    assert_eq!(envelope.metadata(), &());
}
