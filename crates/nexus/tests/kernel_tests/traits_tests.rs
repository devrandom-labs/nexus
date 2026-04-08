use nexus::*;
use std::fmt;

// --- Test ID type ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for TestId {}

// --- Test Events ---
#[derive(Debug, Clone)]
struct ThingCreated {
    name: String,
}

#[derive(Debug, Clone)]
struct ThingActivated;

#[derive(Debug, Clone)]
enum ThingEvent {
    Created(ThingCreated),
    Activated(ThingActivated),
}

impl Message for ThingEvent {}
impl DomainEvent for ThingEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "ThingCreated",
            Self::Activated(_) => "ThingActivated",
        }
    }
}

// --- Test State ---
#[derive(Default, Debug, Clone)]
struct ThingState {
    name: String,
    active: bool,
}

impl AggregateState for ThingState {
    type Event = ThingEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &ThingEvent) -> Self {
        match event {
            ThingEvent::Created(e) => self.name.clone_from(&e.name),
            ThingEvent::Activated(_) => self.active = true,
        }
        self
    }
}

// --- Test Aggregate ---
struct ThingAggregate;

#[derive(Debug, thiserror::Error)]
#[allow(
    dead_code,
    reason = "test-only error type; variants used for type checking"
)]
enum ThingError {
    #[error("already exists")]
    AlreadyExists,
}

impl Aggregate for ThingAggregate {
    type State = ThingState;
    type Error = ThingError;
    type Id = TestId;
}

// --- Tests ---
#[test]
fn event_of_resolves_correctly() {
    fn assert_event_type<A: Aggregate>()
    where
        EventOf<A>: DomainEvent,
    {
    }
    assert_event_type::<ThingAggregate>();
}

#[test]
fn domain_event_name_works() {
    let event = ThingEvent::Created(ThingCreated {
        name: "test".into(),
    });
    assert_eq!(event.name(), "ThingCreated");
    let event = ThingEvent::Activated(ThingActivated);
    assert_eq!(event.name(), "ThingActivated");
}

#[test]
fn aggregate_state_apply_mutates() {
    let state = ThingState::default();
    let state = state.apply(&ThingEvent::Created(ThingCreated {
        name: "hello".into(),
    }));
    assert_eq!(state.name, "hello");
    assert!(!state.active);
    let state = state.apply(&ThingEvent::Activated(ThingActivated));
    assert!(state.active);
}
