/// Replaying an event from one aggregate into a different aggregate's root
/// must fail at compile time. This is the core type safety guarantee.

use nexus::*;

// --- User domain ---
#[derive(Debug, Clone)]
enum UserEvent { Created }
impl Message for UserEvent {}
impl DomainEvent for UserEvent {
    fn name(&self) -> &'static str { "Created" }
}

#[derive(Default, Debug, Clone)]
struct UserState;
impl AggregateState for UserState {
    type Event = UserEvent;
    fn initial() -> Self { Self::default() }
    fn apply(self, _: &UserEvent) -> Self { self }
}

// --- Order domain (different!) ---
#[derive(Debug, Clone)]
enum OrderEvent { Placed }
impl Message for OrderEvent {}
impl DomainEvent for OrderEvent {
    fn name(&self) -> &'static str { "Placed" }
}

// --- Aggregate ---
#[derive(Debug)]
struct UserAggregate;

#[derive(Debug, thiserror::Error)]
#[error("err")]
struct UserError;

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UserId(u64);
impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl Id for UserId {}

fn main() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId(1));
    // This MUST fail: OrderEvent is not UserEvent
    user.replay(Version::INITIAL, &OrderEvent::Placed);
}
