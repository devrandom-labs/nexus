use super::{BoxedEvent, metadata::EventMetadata, pending::PendingEvent};
use crate::{
    domain::Id,
    error::{Error, Result},
};
use std::num::NonZeroU64;

pub struct PendingEventBuilder<E>
where
    E: PendingEventState,
{
    state: E,
}

impl<I> PendingEventBuilder<WithStreamId<I>>
where
    I: Id + Ord,
{
    pub fn new(stream_id: I) -> Self {
        let state = WithStreamId { stream_id };
        PendingEventBuilder { state }
    }

    pub fn with_version(self, version: u64) -> Result<PendingEventBuilder<WithVersion<I>>> {
        let version = NonZeroU64::new(version).ok_or_else(|| Error::InvalidArgument {
            name: "version".to_string(),
            reason: "must be greater than 0".to_string(),
            context: "PendingEventBuilder::with_version".to_string(),
        })?;

        let state = WithVersion {
            stream_id: self.state.stream_id,
            version,
        };

        Ok(PendingEventBuilder { state })
    }
}

impl<I> PendingEventBuilder<WithVersion<I>>
where
    I: Id + Ord,
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
    I: Id + Ord,
{
    #[cfg(feature = "testing")]
    pub fn build_with_payload(
        self,
        payload: Vec<u8>,
        event_type: String,
    ) -> Result<PendingEvent<I>> {
        let WithMetadata {
            stream_id,
            version,
            metadata,
        } = self.state;
        let pending_event = PendingEvent::new(stream_id, version, event_type, metadata, payload)?;
        Ok(pending_event)
    }

    pub fn with_domain(self, domain_event: BoxedEvent) -> PendingEventBuilder<WithDomain<I>> {
        let state = WithDomain {
            stream_id: self.state.stream_id,
            version: self.state.version,
            event_type: domain_event.name().to_string(),
            domain_event,
            metadata: self.state.metadata,
        };

        PendingEventBuilder { state }
    }
}

impl<I> PendingEventBuilder<WithDomain<I>>
where
    I: Id + Ord,
{
    pub async fn build<F, Fut>(self, serializer: F) -> Result<PendingEvent<I>>
    where
        F: FnOnce(BoxedEvent) -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let WithDomain {
            stream_id,
            domain_event,
            version,
            event_type,
            metadata,
        } = self.state;
        let payload = serializer(domain_event).await?;
        let pending_event = PendingEvent::new(stream_id, version, event_type, metadata, payload)?;
        Ok(pending_event)
    }
}

pub trait PendingEventState {}

pub struct WithStreamId<I>
where
    I: Id + Ord,
{
    stream_id: I,
}

impl<I> PendingEventState for WithStreamId<I> where I: Id + Ord {}

pub struct WithVersion<I>
where
    I: Id + Ord,
{
    stream_id: I,
    version: NonZeroU64,
}

impl<I> PendingEventState for WithVersion<I> where I: Id + Ord {}

pub struct WithMetadata<I>
where
    I: Id + Ord,
{
    stream_id: I,
    version: NonZeroU64,
    metadata: EventMetadata,
}

impl<I> PendingEventState for WithMetadata<I> where I: Id + Ord {}

pub struct WithDomain<I>
where
    I: Id + Ord,
{
    stream_id: I,
    event_type: String,
    domain_event: BoxedEvent,
    version: NonZeroU64,
    metadata: EventMetadata,
}

impl<I> PendingEventState for WithDomain<I> where I: Id + Ord {}
