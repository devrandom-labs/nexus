use nexus::Events;
use nexus::*;

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
            Self::Created(_) => "Created",
            Self::Activated(_) => "Activated",
        }
    }
}

#[test]
fn versioned_event_holds_version_and_event() {
    let v1 = Version::new(1).unwrap();
    let ve = VersionedEvent::new(v1, TestEvent::Created(Created));
    assert_eq!(ve.version(), v1);
    assert_eq!(ve.event(), &TestEvent::Created(Created));
}

#[test]
fn events_guarantees_non_empty() {
    let events: Events<_, 0> = Events::new(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
    assert!(!events.is_empty());
}

#[test]
fn events_add_increases_len() {
    let mut events: Events<_, 1> = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    assert_eq!(events.len(), 2);
}

#[test]
fn events_into_iter() {
    let mut events: Events<_, 1> = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    let collected: Vec<_> = events.into_iter().collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0], TestEvent::Created(Created));
    assert_eq!(collected[1], TestEvent::Activated(Activated));
}

#[test]
fn events_from_single() {
    let events: Events<_, 0> = Events::from(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
}

#[test]
fn events_macro_single() {
    let events: Events<_, 0> = nexus::events![TestEvent::Created(Created)];
    assert_eq!(events.len(), 1);
}

#[test]
fn events_macro_multiple() {
    let events: Events<_, 1> =
        nexus::events![TestEvent::Created(Created), TestEvent::Activated(Activated),];
    assert_eq!(events.len(), 2);
}

#[test]
#[should_panic(expected = "Events capacity exceeded")]
fn add_panics_on_capacity_overflow() {
    let mut events: Events<_, 0> = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated)); // N=0, no room for additional events
}
