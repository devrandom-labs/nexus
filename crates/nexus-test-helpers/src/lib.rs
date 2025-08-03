use fake::{Dummy, Fake, Faker};
use nexus::{
    event::{BoxedEvent, PendingEvent, PersistedEvent},
    infra::NexusId,
};

pub mod pending_event;
pub mod user_domain;

// This `TestableEvent` IS defined in the current crate.
#[derive(Debug, Clone)]
pub struct TestableEvent(pub PendingEvent<NexusId>);

impl PartialEq<PersistedEvent<NexusId>> for TestableEvent {
    fn eq(&self, other: &PersistedEvent<NexusId>) -> bool {
        self.0.id() == &other.id
            && self.0.stream_id() == &other.stream_id
            && self.0.event_type() == other.event_type
            && self.0.version().get() == other.version
            && self.0.payload() == other.payload
            && self.0.metadata().correlation_id() == other.metadata.correlation_id()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UserEvents {
    Created(user_domain::UserCreated),
    NameUpdated(user_domain::UserNameUpdated),
    Activated(user_domain::UserActivated),
    Deactivated(user_domain::UserDeactivated),
}

impl From<UserEvents> for BoxedEvent {
    fn from(value: UserEvents) -> Self {
        match value {
            UserEvents::Activated(e) => Box::new(e),
            UserEvents::Created(e) => Box::new(e),
            UserEvents::NameUpdated(e) => Box::new(e),
            UserEvents::Deactivated(e) => Box::new(e),
        }
    }
}

impl Dummy<Faker> for UserEvents {
    fn dummy_with_rng<R: fake::Rng + ?Sized>(_config: &Faker, rng: &mut R) -> Self {
        let variant = rng.random_range(0..4_u8);
        match variant {
            0 => Self::Created(Faker.fake()),
            1 => Self::Activated(Faker.fake()),
            2 => Self::NameUpdated(Faker.fake()),
            _ => Self::Deactivated(Faker.fake()),
        }
    }
}
