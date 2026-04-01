//! Integration test: the complete kernel user experience.
//! Uses #[derive(DomainEvent)] macro + full aggregate lifecycle.
//!
//! Note: Business logic is written as free functions because
//! `impl AggregateRoot<UserAggregate>` requires inherent impl in the
//! defining crate. In production, `#[derive(Aggregate)]` generates
//! a wrapper type that users own.

use nexus::AggregateRoot;
use nexus::VersionedEvent;
use nexus::*;
use std::fmt;

// --- ID ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UserId(u64);
impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "user-{}", self.0)
    }
}
impl Id for UserId {}

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
#[derive(Default, Debug)]
struct UserState {
    name: String,
    active: bool,
}

impl AggregateState for UserState {
    type Event = UserEvent;
    fn initial() -> Self { Self::default() }
    fn apply(&mut self, event: &UserEvent) {
        match event {
            UserEvent::Created(e) => self.name.clone_from(&e.name),
            UserEvent::Activated(_) => self.active = true,
        }
    }
    fn name(&self) -> &'static str {
        "User"
    }
}

// --- Aggregate ---
struct UserAggregate;

#[derive(Debug, thiserror::Error)]
enum UserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user already active")]
    AlreadyActive,
}

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

// --- Business logic as free functions (see note above) ---
fn create_user(agg: &mut AggregateRoot<UserAggregate>, name: String) -> Result<(), UserError> {
    if !agg.state().name.is_empty() {
        return Err(UserError::AlreadyExists);
    }
    agg.apply_event(UserEvent::Created(UserCreated { name }));
    Ok(())
}

fn activate_user(agg: &mut AggregateRoot<UserAggregate>) -> Result<(), UserError> {
    if agg.state().active {
        return Err(UserError::AlreadyActive);
    }
    agg.apply_event(UserEvent::Activated(UserActivated));
    Ok(())
}

// --- Tests ---

#[test]
fn full_aggregate_lifecycle() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId(1));

    create_user(&mut user, "Alice".into()).unwrap();
    activate_user(&mut user).unwrap();

    assert_eq!(user.state().name, "Alice");
    assert!(user.state().active);
    assert_eq!(user.current_version(), Version::from_persisted(2));

    let events = user.take_uncommitted_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].version(), Version::from_persisted(1));
    assert_eq!(events[1].version(), Version::from_persisted(2));
    assert_eq!(events[0].event().name(), "Created");
    assert_eq!(events[1].event().name(), "Activated");
}

#[test]
fn rehydrate_then_continue() {
    let history = vec![VersionedEvent::from_persisted(
        Version::from_persisted(1),
        UserEvent::Created(UserCreated { name: "Bob".into() }),
    )];
    let mut user = AggregateRoot::<UserAggregate>::load_from_events(UserId(2), history).unwrap();

    assert_eq!(user.version(), Version::from_persisted(1));
    assert_eq!(user.state().name, "Bob");

    activate_user(&mut user).unwrap();
    let events = user.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].version(), Version::from_persisted(2));
}

#[test]
fn invariant_violations_return_domain_errors() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId(3));
    create_user(&mut user, "Charlie".into()).unwrap();

    assert!(matches!(
        create_user(&mut user, "David".into()),
        Err(UserError::AlreadyExists)
    ));

    activate_user(&mut user).unwrap();
    assert!(matches!(
        activate_user(&mut user),
        Err(UserError::AlreadyActive)
    ));
}
