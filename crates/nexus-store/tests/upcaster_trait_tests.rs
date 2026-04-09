#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::Version;
use nexus_store::Upcaster;
use nexus_store::upcasting::EventMorsel;

#[test]
fn unit_upcaster_is_passthrough() {
    let upcaster = ();
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"payload");
    let result = upcaster.apply(morsel).unwrap();
    assert_eq!(result.event_type(), "OrderCreated");
    assert_eq!(result.schema_version(), Version::INITIAL);
    assert_eq!(result.payload(), b"payload");
    assert!(result.is_borrowed(), "passthrough should stay borrowed");
}

#[test]
fn unit_upcaster_current_version_is_none() {
    let upcaster = ();
    assert_eq!(upcaster.current_version("OrderCreated"), None);
}
