use super::builder::{PendingEventBuilder, WithDomain};
use super::metadata::EventMetadata;
use crate::{
    domain::{DomainEvent, Id},
    infra::EventId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEvent<I>
where
    I: Id,
{
    id: EventId,
    stream_id: I,
    version: u64,
    event_type: String,
    metadata: EventMetadata,
    payload: Vec<u8>,
}

impl<I> PendingEvent<I>
where
    I: Id,
{
    pub(crate) fn new(
        stream_id: I,
        version: u64,
        event_type: String,
        metadata: EventMetadata,
        payload: Vec<u8>,
    ) -> Self {
        PendingEvent {
            id: EventId::default(),
            stream_id,
            version,
            event_type,
            metadata,
            payload,
        }
    }

    pub fn builder<D>(domain_event: D) -> PendingEventBuilder<WithDomain<D, I>>
    where
        D: DomainEvent,
    {
        PendingEventBuilder::new(domain_event)
    }
}

impl<I> PendingEvent<I>
where
    I: Id,
{
    pub fn stream_id(&self) -> &I {
        &self.stream_id
    }

    pub fn id(&self) -> &EventId {
        &self.id
    }

    pub fn version(&self) -> &u64 {
        &self.version
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn metadata(&self) -> &EventMetadata {
        &self.metadata
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
