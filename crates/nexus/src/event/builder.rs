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

impl<I> PendingEventBuilder<WithStreamId<I>>
where
    I: Id,
{
    pub fn new(stream_id: I) -> Self {
        let state = WithStreamId { stream_id };
        PendingEventBuilder { state }
    }

    pub fn with_version(self, version: u64) -> PendingEventBuilder<WithVersion<I>> {
        let state = WithVersion {
            stream_id: self.state.stream_id,

            version,
        };

        PendingEventBuilder { state }
    }
}

impl<I> PendingEventBuilder<WithVersion<I>>
where
    I: Id,
{
    pub fn with_metadata(self, metadata: EventMetadata) -> PendingEventBuilder<WithMetadata<I>> {
        let state = WithMetadata {
            stream_id: self.state.stream_id,
            version: self.state.version,
            metadata,
        };

        PendingEventBuilder { state }
    }
}

impl<I> PendingEventBuilder<WithMetadata<I>>
where
    I: Id,
{
    pub async fn with_domain<D>(self, domain_event: D) -> PendingEventBuilder<WithDomain<I, D>>
    where
        D: DomainEvent,
    {
        let state = WithDomain {
            stream_id: self.state.stream_id,
            version: self.state.version,
            event_type: &domain_event.clone(),
            domain_event,
            metadata,
        };

        PendingEventBuilder { state }
    }
}

impl<I, D> PendingEventBuilder<WithDomain<I, D>>
where
    I: Id,
    D: DomainEvent,
{
    pub async fn build<F, Fut>(self, serializer: F) -> Result<PendingEvent<I>, Error>
    where
        F: FnOnce(D) -> Fut,
        Fut: Future<Output = Result<Vec<u8>, Error>>,
    {
        let WithDomain {
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

pub struct WithStreamId<I>
where
    I: Id,
{
    stream_id: I,
}

impl<I> PendingEventState for WithStreamId<I> where I: Id {}

pub struct WithVersion<I>
where
    I: Id,
{
    stream_id: I,
    version: u64,
}

impl<I> PendingEventState for WithVersion<I> where I: Id {}

pub struct WithMetadata<I>
where
    I: Id,
{
    stream_id: I,
    version: u64,
    metadata: EventMetadata,
}

impl<I> PendingEventState for WithMetadata<I> where I: Id {}

pub struct WithDomain<D, I>
where
    D: DomainEvent,
    I: Id,
{
    stream_id: I,
    event_type: String,
    domain_event: D,
    version: u64,
    metadata: EventMetadata,
}

impl<D, I> PendingEventState for WithDomain<D, I>
where
    D: DomainEvent,
    I: Id,
{
}
