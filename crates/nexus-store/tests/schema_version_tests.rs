use nexus::Version;
use nexus_store::PersistedEnvelope;

#[test]
fn persisted_envelope_stores_schema_version() {
    let env = PersistedEnvelope::new(
        "stream-1",
        Version::from_persisted(1),
        "UserCreated",
        1,
        b"payload",
        (),
    );
    assert_eq!(env.schema_version(), 1);
}

#[test]
fn persisted_envelope_schema_version_max() {
    let env = PersistedEnvelope::new("s", Version::from_persisted(1), "E", u32::MAX, b"p", ());
    assert_eq!(env.schema_version(), u32::MAX);
}

#[test]
#[should_panic(expected = "schema_version must be > 0")]
fn persisted_envelope_rejects_zero_schema_version() {
    let _ = PersistedEnvelope::new("s", Version::from_persisted(1), "E", 0, b"p", ());
}
