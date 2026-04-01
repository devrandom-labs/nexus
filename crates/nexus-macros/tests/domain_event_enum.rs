#![allow(dead_code)]

use nexus::{DomainEvent, Message};

// Test with all variant shapes: tuple, struct, and unit
#[derive(Debug, Clone, nexus::DomainEvent)]
enum TestEvent {
    Created(Created),
    Updated(Updated),
    Renamed { new_name: String },
    Deleted,
}

#[derive(Debug, Clone)]
struct Created {
    name: String,
}

#[derive(Debug, Clone)]
struct Updated {
    name: String,
}

#[test]
fn derive_domain_event_on_enum_generates_name() {
    let e = TestEvent::Created(Created {
        name: "test".into(),
    });
    assert_eq!(e.name(), "Created");

    let e = TestEvent::Updated(Updated { name: "new".into() });
    assert_eq!(e.name(), "Updated");

    let e = TestEvent::Renamed {
        new_name: "renamed".into(),
    };
    assert_eq!(e.name(), "Renamed");

    let e = TestEvent::Deleted;
    assert_eq!(e.name(), "Deleted");
}

#[test]
fn derive_domain_event_implements_message() {
    fn assert_message<T: Message>() {}
    assert_message::<TestEvent>();
}

#[test]
fn derive_domain_event_implements_domain_event() {
    fn assert_domain_event<T: DomainEvent>() {}
    assert_domain_event::<TestEvent>();
}
