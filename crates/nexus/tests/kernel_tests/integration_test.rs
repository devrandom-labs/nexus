//! Integration test: the complete kernel user experience.
//! Uses #[derive(DomainEvent)] macro + full aggregate lifecycle
//! with the Handle/decide pattern.

use nexus::*;
use std::fmt;

// --- ID ---
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

// --- Events (using derive macro!) ---
#[derive(Debug, Clone)]
struct UserCreated {
    name: String,
}

#[derive(Debug, Clone)]
struct UserActivated;

#[derive(Debug, Clone, nexus_macros::DomainEvent)]
enum UserEvent {
    Created(UserCreated),
    Activated(UserActivated),
}

// --- State ---
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

// --- Aggregate newtype ---
struct User(AggregateRoot<Self>);

#[derive(Debug, thiserror::Error)]
enum UserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user already active")]
    AlreadyActive,
}

impl Aggregate for User {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

impl AggregateEntity for User {
    fn root(&self) -> &AggregateRoot<Self> {
        &self.0
    }
    fn root_mut(&mut self) -> &mut AggregateRoot<Self> {
        &mut self.0
    }
}

impl User {
    fn new(id: UserId) -> Self {
        Self(AggregateRoot::new(id))
    }
}

// --- Commands ---
struct CreateUser {
    name: String,
}
struct ActivateUser;

// --- Command handlers (decide pattern) ---
impl Handle<CreateUser> for User {
    fn handle(&self, cmd: CreateUser) -> Result<Events<UserEvent>, UserError> {
        if !self.state().name.is_empty() {
            return Err(UserError::AlreadyExists);
        }
        Ok(events![UserEvent::Created(UserCreated { name: cmd.name })])
    }
}

impl Handle<ActivateUser> for User {
    fn handle(&self, _cmd: ActivateUser) -> Result<Events<UserEvent>, UserError> {
        if self.state().active {
            return Err(UserError::AlreadyActive);
        }
        Ok(events![UserEvent::Activated(UserActivated)])
    }
}

// --- Tests ---

#[test]
fn full_aggregate_lifecycle_with_handle() {
    let mut user = User::new(UserId::new(1));

    // Decide: command produces events
    let create_events = user
        .handle(CreateUser {
            name: "Alice".into(),
        })
        .unwrap();
    assert_eq!(create_events.len(), 1);
    assert_eq!(create_events.iter().next().unwrap().name(), "Created");

    // Simulate persistence + state advancement
    let v1 = Version::new(1).unwrap();
    user.root_mut().advance_version(v1);
    user.root_mut().apply_events(&create_events);

    assert_eq!(user.state().name, "Alice");
    assert_eq!(user.version(), Some(v1));

    // Second command
    let activate_events = user.handle(ActivateUser).unwrap();
    let v2 = Version::new(2).unwrap();
    user.root_mut().advance_version(v2);
    user.root_mut().apply_events(&activate_events);

    assert!(user.state().active);
    assert_eq!(user.version(), Some(v2));
}

#[test]
fn rehydrate_then_decide() {
    let mut user = User::new(UserId::new(2));
    user.replay(
        Version::new(1).unwrap(),
        &UserEvent::Created(UserCreated { name: "Bob".into() }),
    )
    .unwrap();

    assert_eq!(user.version(), Some(Version::new(1).unwrap()));
    assert_eq!(user.state().name, "Bob");

    // Decide after rehydration
    let events = user.handle(ActivateUser).unwrap();
    assert_eq!(events.len(), 1);

    // Persist and advance
    let v2 = Version::new(2).unwrap();
    user.root_mut().advance_version(v2);
    user.root_mut().apply_events(&events);

    assert!(user.state().active);
    assert_eq!(user.version(), Some(v2));
}

#[test]
fn invariant_violations_return_domain_errors() {
    let mut user = User::new(UserId::new(3));

    // Create the user
    let events = user
        .handle(CreateUser {
            name: "Charlie".into(),
        })
        .unwrap();
    user.root_mut().advance_version(Version::new(1).unwrap());
    user.root_mut().apply_events(&events);

    // Duplicate creation rejected
    assert!(matches!(
        user.handle(CreateUser {
            name: "David".into()
        }),
        Err(UserError::AlreadyExists)
    ));

    // Activate the user
    let events = user.handle(ActivateUser).unwrap();
    user.root_mut().advance_version(Version::new(2).unwrap());
    user.root_mut().apply_events(&events);

    // Duplicate activation rejected
    assert!(matches!(
        user.handle(ActivateUser),
        Err(UserError::AlreadyActive)
    ));
}

#[test]
fn derive_macro_generates_correct_event_names() {
    let created = UserEvent::Created(UserCreated {
        name: "test".into(),
    });
    let activated = UserEvent::Activated(UserActivated);

    assert_eq!(created.name(), "Created");
    assert_eq!(activated.name(), "Activated");
}
