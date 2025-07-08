#![allow(dead_code)]
use super::{metadata::EventMetadata, pending::PendingEvent};
use crate::{
    domain::{DomainEvent, Id},
    error::Error,
};

pub struct PendingEventBuilder<E>
where
    E: PendingEventState,
{
    state: E,
}

impl<D, I> PendingEventBuilder<WithDomain<D, I>>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub fn new(domain_event: D) -> Self {
        let state = WithDomain {
            stream_id: domain_event.aggregate_id().clone(),
            event_type: domain_event.name().to_owned(),
            domain_event,
        };
        PendingEventBuilder { state }
    }

    pub fn with_version(self, version: u64) -> PendingEventBuilder<WithVersion<D, I>> {
        let state = WithVersion {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            event_type: self.state.event_type,
            version,
        };

        PendingEventBuilder { state }
    }
}

impl<D, I> PendingEventBuilder<WithVersion<D, I>>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
    pub fn with_metadata(self, metadata: EventMetadata) -> PendingEventBuilder<WithMetadata<D, I>> {
        let state = WithMetadata {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            event_type: self.state.event_type,
            metadata,
        };

        PendingEventBuilder { state }
    }
}

impl<D, I> PendingEventBuilder<WithMetadata<D, I>>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
    pub async fn serialize<F, Fut>(self, serializer: F) -> Result<PendingEvent<I>, Error>
    where
        F: FnOnce(D) -> Fut,
        Fut: Future<Output = Result<Vec<u8>, Error>>,
    {
        let WithMetadata {
            stream_id,
            domain_event,
            version,
            event_type,
            metadata,
        } = self.state;
        let payload = serializer(domain_event).await?;
        Ok(PendingEvent::new(
            stream_id, version, event_type, metadata, payload,
        ))
    }
}

pub trait PendingEventState {}

pub struct WithDomain<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
    stream_id: I,
    event_type: String,
    domain_event: D,
}

impl<D, I> PendingEventState for WithDomain<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
}

pub struct WithVersion<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
    version: u64,
    stream_id: I,
    event_type: String,
    domain_event: D,
}

impl<D, I> PendingEventState for WithVersion<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
}

pub struct WithMetadata<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
    stream_id: I,
    domain_event: D,
    event_type: String,
    version: u64,
    metadata: EventMetadata,
}

impl<D, I> PendingEventState for WithMetadata<D, I>
where
    D: DomainEvent<Id = I>,
    I: Id,
{
}
