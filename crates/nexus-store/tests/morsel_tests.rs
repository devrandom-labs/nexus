use std::borrow::Cow;

use nexus::Version;
use nexus_store::upcasting::EventMorsel;

#[test]
fn morsel_from_borrowed_is_zero_alloc() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"some bytes");
    assert_eq!(morsel.event_type(), "OrderCreated");
    assert_eq!(morsel.schema_version(), Version::INITIAL);
    assert_eq!(morsel.payload(), b"some bytes");
    assert!(morsel.is_borrowed());
}

#[test]
fn morsel_with_transformed_payload() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"old");
    let transformed = morsel.with_payload(Cow::Owned(b"new".to_vec()));
    assert_eq!(transformed.payload(), b"new");
    assert!(!transformed.is_borrowed());
}

#[test]
fn morsel_with_advanced_version() {
    let v2 = Version::new(2).expect("2 is nonzero");
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let upgraded = morsel.with_schema_version(v2);
    assert_eq!(upgraded.schema_version(), v2);
}

#[test]
fn morsel_with_renamed_event_type() {
    let morsel = EventMorsel::borrowed("OldName", Version::INITIAL, b"data");
    let renamed = morsel.with_event_type(Cow::Owned("NewName".to_owned()));
    assert_eq!(renamed.event_type(), "NewName");
}

#[test]
fn morsel_passthrough_stays_borrowed() {
    let morsel = EventMorsel::borrowed("Test", Version::INITIAL, b"data");
    let passthrough = morsel.with_schema_version(Version::INITIAL); // no real change
    assert!(passthrough.is_borrowed()); // payload and event_type unchanged
}

#[test]
fn morsel_new_creates_owned() {
    let morsel = EventMorsel::new("OrderCreated", Version::INITIAL, vec![1, 2, 3]);
    assert_eq!(morsel.event_type(), "OrderCreated");
    assert_eq!(morsel.schema_version(), Version::INITIAL);
    assert_eq!(morsel.payload(), &[1, 2, 3]);
    assert!(!morsel.is_borrowed(), "new() creates owned morsel");
}
