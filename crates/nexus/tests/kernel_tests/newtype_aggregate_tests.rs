//! Tests the marker aggregate + dispatch pattern — what `#[nexus::aggregate]`
//! generates. The aggregate is a bare marker; command handlers are pure
//! associated functions on the marker, dispatched via `AggregateRoot::handle`.

use nexus::*;
use std::fmt;

// --- Domain types ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UserId(String);
impl UserId {
    fn new(v: u64) -> Self {
        Self(format!("user-{v}"))
    }
}
impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl AsRef<[u8]> for UserId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for UserId {
    const BYTE_LEN: usize = 0;
}

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

// --- This is what #[nexus::aggregate] would generate: a bare marker ---

struct UserAggregate;

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

// --- Commands ---
struct CreateUser {
    name: String,
}
struct ActivateUser;

// --- Command handlers (decide pattern) — the user writes this ---
impl Handle<CreateUser> for UserAggregate {
    fn handle(state: &UserState, cmd: CreateUser) -> Result<Events<UserEvent>, UserError> {
        if !state.name.is_empty() {
            return Err(UserError::AlreadyExists);
        }
        Ok(events![UserEvent::Created(UserCreated { name: cmd.name })])
    }
}

impl Handle<ActivateUser> for UserAggregate {
    fn handle(state: &UserState, _cmd: ActivateUser) -> Result<Events<UserEvent>, UserError> {
        if state.active {
            return Err(UserError::AlreadyActive);
        }
        Ok(events![UserEvent::Activated(UserActivated)])
    }
}

// --- Tests ---

#[test]
fn marker_aggregate_lifecycle() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId::new(1));

    let events = user
        .handle(CreateUser {
            name: "Alice".into(),
        })
        .unwrap();
    let v1 = Version::new(1).unwrap();
    user.commit_persisted(v1, &events);

    let events = user.handle(ActivateUser).unwrap();
    let v2 = Version::new(2).unwrap();
    user.commit_persisted(v2, &events);

    assert_eq!(user.state().name, "Alice");
    assert!(user.state().active);
    assert_eq!(user.version(), Some(v2));
}

#[test]
fn marker_aggregate_invariant_enforcement() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId::new(2));

    let events = user.handle(CreateUser { name: "Bob".into() }).unwrap();
    user.commit_persisted(Version::new(1).unwrap(), &events);

    assert!(matches!(
        user.handle(CreateUser {
            name: "Charlie".into()
        }),
        Err(UserError::AlreadyExists)
    ));

    let events = user.handle(ActivateUser).unwrap();
    user.commit_persisted(Version::new(2).unwrap(), &events);

    assert!(matches!(
        user.handle(ActivateUser),
        Err(UserError::AlreadyActive)
    ));
}

#[test]
fn marker_aggregate_rehydrate() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId::new(3));
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
fn marker_aggregate_id_accessible() {
    let user = AggregateRoot::<UserAggregate>::new(UserId::new(42));
    assert_eq!(user.id(), &UserId::new(42));
}

#[test]
fn marker_aggregate_initial_state() {
    let user = AggregateRoot::<UserAggregate>::new(UserId::new(1));
    assert_eq!(user.state().name, "");
    assert!(!user.state().active);
}

#[test]
fn marker_aggregate_fresh_version_is_none() {
    let user = AggregateRoot::<UserAggregate>::new(UserId::new(1));
    assert_eq!(user.version(), None);
}
