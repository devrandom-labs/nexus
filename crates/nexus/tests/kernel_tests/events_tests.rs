use nexus::kernel::events::Events;
use nexus::kernel::*;

#[derive(Debug, Clone, PartialEq)]
struct Created;
#[derive(Debug, Clone, PartialEq)]
struct Activated;

#[derive(Debug, Clone, PartialEq)]
enum TestEvent {
    Created(Created),
    Activated(Activated),
}
impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Created(_) => "Created",
            TestEvent::Activated(_) => "Activated",
        }
    }
}

#[test]
fn versioned_event_holds_version_and_event() {
    let ve = VersionedEvent::from_persisted(Version::from_persisted(1), TestEvent::Created(Created));
    assert_eq!(ve.version(), Version::from_persisted(1));
    assert_eq!(ve.event(), &TestEvent::Created(Created));
}

#[test]
fn events_guarantees_non_empty() {
    let events = Events::new(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
    assert!(!events.is_empty());
}

#[test]
fn events_add_increases_len() {
    let mut events = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    assert_eq!(events.len(), 2);
}

#[test]
fn events_into_iter() {
    let mut events = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    let collected: Vec<_> = events.into_iter().collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0], TestEvent::Created(Created));
    assert_eq!(collected[1], TestEvent::Activated(Activated));
}

#[test]
fn events_from_single() {
    let events = Events::from(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
}

#[test]
fn events_macro_single() {
    let events = nexus::events![TestEvent::Created(Created)];
    assert_eq!(events.len(), 1);
}

#[test]
fn events_macro_multiple() {
    let events = nexus::events![TestEvent::Created(Created), TestEvent::Activated(Activated),];
    assert_eq!(events.len(), 2);
}
