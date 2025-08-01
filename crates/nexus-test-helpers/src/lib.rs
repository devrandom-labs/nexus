use nexus::{
    event::{PendingEvent, PersistedEvent},
    infra::NexusId,
};

pub mod pending_event_strategies;
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
