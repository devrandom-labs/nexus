//! Tests the newtype aggregate pattern — what #[derive(Aggregate)] will generate.
//! This tests the design manually before writing the macro.

use nexus::*;
use std::fmt;

// --- Domain types ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UserId(u64);
impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "user-{}", self.0)
    }
}
impl Id for UserId {}

#[derive(Debug, Clone)]
struct UserCreated {
    name: String,
}
#[derive(Debug, Clone)]
struct UserActivated;

#[derive(Debug, Clone)]
enum UserEvent {
    Created(UserCreated),
    Activated(UserActivated),
}
impl Message for UserEvent {}
impl DomainEvent for UserEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "UserCreated",
            Self::Activated(_) => "UserActivated",
        }
    }
}

#[derive(Default, Debug)]
struct UserState {
    name: String,
    active: bool,
}
impl AggregateState for UserState {
    type Event = UserEvent;
    fn apply(&mut self, event: &UserEvent) {
        match event {
            UserEvent::Created(e) => self.name = e.name.clone(),
            UserEvent::Activated(_) => self.active = true,
        }
    }
    fn name(&self) -> &'static str {
        "User"
    }
}

#[derive(Debug, thiserror::Error)]
enum UserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user already active")]
    AlreadyActive,
}

// --- This is what #[derive(Aggregate)] would generate ---

struct UserAggregate(AggregateRoot<Self>);

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

impl AggregateEntity for UserAggregate {
    fn root(&self) -> &AggregateRoot<Self> {
        &self.0
    }
    fn root_mut(&mut self) -> &mut AggregateRoot<Self> {
        &mut self.0
    }
}

impl UserAggregate {
    fn new(id: UserId) -> Self {
        Self(AggregateRoot::new(id))
    }
}

// --- Business logic — the user writes this ---

impl UserAggregate {
    fn create(&mut self, name: String) -> Result<(), UserError> {
        if !self.state().name.is_empty() {
            return Err(UserError::AlreadyExists);
        }
        self.apply_event(UserEvent::Created(UserCreated { name }));
        Ok(())
    }

    fn activate(&mut self) -> Result<(), UserError> {
        if self.state().active {
            return Err(UserError::AlreadyActive);
        }
        self.apply_event(UserEvent::Activated(UserActivated));
        Ok(())
    }
}

// --- Tests ---

#[test]
fn newtype_aggregate_lifecycle() {
    let mut user = UserAggregate::new(UserId(1));
    user.create("Alice".into()).unwrap();
    user.activate().unwrap();

    assert_eq!(user.state().name, "Alice");
    assert!(user.state().active);
    assert_eq!(user.current_version(), Version::from_persisted(2));

    let events = user.take_uncommitted_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].version(), Version::from_persisted(1));
    assert_eq!(events[1].version(), Version::from_persisted(2));
}

#[test]
fn newtype_aggregate_invariant_enforcement() {
    let mut user = UserAggregate::new(UserId(2));
    user.create("Bob".into()).unwrap();

    assert!(matches!(
        user.create("Charlie".into()),
        Err(UserError::AlreadyExists)
    ));

    user.activate().unwrap();
    assert!(matches!(
        user.activate(),
        Err(UserError::AlreadyActive)
    ));
}

#[test]
fn newtype_aggregate_rehydrate() {
    let history = vec![
        VersionedEvent::from_persisted(
            Version::from_persisted(1),
            UserEvent::Created(UserCreated {
                name: "Dave".into(),
            }),
        ),
        VersionedEvent::from_persisted(
            Version::from_persisted(2),
            UserEvent::Activated(UserActivated),
        ),
    ];
    let user = UserAggregate(AggregateRoot::load_from_events(UserId(3), history).unwrap());

    assert_eq!(user.state().name, "Dave");
    assert!(user.state().active);
    assert_eq!(user.version(), Version::from_persisted(2));
}

#[test]
fn newtype_aggregate_id_accessible() {
    let user = UserAggregate::new(UserId(42));
    assert_eq!(user.id(), &UserId(42));
}
