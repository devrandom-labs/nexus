//! Tests the newtype aggregate pattern — what `#[nexus::aggregate]` will generate.
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

#[derive(Default, Debug, Clone)]
struct UserState {
    name: String,
    active: bool,
}
impl AggregateState for UserState {
    type Event = UserEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &UserEvent) -> Self {
        match event {
            UserEvent::Created(e) => self.name.clone_from(&e.name),
            UserEvent::Activated(_) => self.active = true,
        }
        self
    }
}

#[derive(Debug, thiserror::Error)]
enum UserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user already active")]
    AlreadyActive,
}

// --- This is what #[nexus::aggregate] would generate ---

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

// --- Commands ---
struct CreateUser {
    name: String,
}
struct ActivateUser;

// --- Command handlers (decide pattern) — the user writes this ---
impl Handle<CreateUser> for UserAggregate {
    fn handle(&self, cmd: CreateUser) -> Result<Events<UserEvent>, UserError> {
        if !self.state().name.is_empty() {
            return Err(UserError::AlreadyExists);
        }
        Ok(events![UserEvent::Created(UserCreated { name: cmd.name })])
    }
}

impl Handle<ActivateUser> for UserAggregate {
    fn handle(&self, _cmd: ActivateUser) -> Result<Events<UserEvent>, UserError> {
        if self.state().active {
            return Err(UserError::AlreadyActive);
        }
        Ok(events![UserEvent::Activated(UserActivated)])
    }
}

// --- Tests ---

#[test]
fn newtype_aggregate_lifecycle() {
    let mut user = UserAggregate::new(UserId(1));

    let events = user
        .handle(CreateUser {
            name: "Alice".into(),
        })
        .unwrap();
    let v1 = Version::new(1).unwrap();
    user.root_mut().advance_version(v1);
    user.root_mut().apply_events(&events);

    let events = user.handle(ActivateUser).unwrap();
    let v2 = Version::new(2).unwrap();
    user.root_mut().advance_version(v2);
    user.root_mut().apply_events(&events);

    assert_eq!(user.state().name, "Alice");
    assert!(user.state().active);
    assert_eq!(user.version(), Some(v2));
}

#[test]
fn newtype_aggregate_invariant_enforcement() {
    let mut user = UserAggregate::new(UserId(2));

    let events = user.handle(CreateUser { name: "Bob".into() }).unwrap();
    user.root_mut().advance_version(Version::new(1).unwrap());
    user.root_mut().apply_events(&events);

    assert!(matches!(
        user.handle(CreateUser {
            name: "Charlie".into()
        }),
        Err(UserError::AlreadyExists)
    ));

    let events = user.handle(ActivateUser).unwrap();
    user.root_mut().advance_version(Version::new(2).unwrap());
    user.root_mut().apply_events(&events);

    assert!(matches!(
        user.handle(ActivateUser),
        Err(UserError::AlreadyActive)
    ));
}

#[test]
fn newtype_aggregate_rehydrate() {
    let mut user = UserAggregate::new(UserId(3));
    user.replay(
        Version::new(1).unwrap(),
        &UserEvent::Created(UserCreated {
            name: "Dave".into(),
        }),
    )
    .unwrap();
    user.replay(
        Version::new(2).unwrap(),
        &UserEvent::Activated(UserActivated),
    )
    .unwrap();

    assert_eq!(user.state().name, "Dave");
    assert!(user.state().active);
    assert_eq!(user.version(), Some(Version::new(2).unwrap()));
}

#[test]
fn newtype_aggregate_id_accessible() {
    let user = UserAggregate::new(UserId(42));
    assert_eq!(user.id(), &UserId(42));
}

#[test]
fn newtype_aggregate_delegates_state() {
    let user = UserAggregate::new(UserId(1));
    assert_eq!(user.state().name, "");
    assert!(!user.state().active);
}

#[test]
fn newtype_aggregate_fresh_version_is_none() {
    let user = UserAggregate::new(UserId(1));
    assert_eq!(user.version(), None);
}
