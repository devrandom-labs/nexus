#![allow(dead_code)]
use super::{StreamId, event_metadata::EventMetadata, event_record::EventRecord};
use crate::{core::DomainEvent, error::Error};

pub struct EventRecordBuilder<E>
where
    E: EventBuilderState,
{
    state: E,
}

impl<D> EventRecordBuilder<WithDomain<D>>
where
    D: DomainEvent,
    D::Id: Into<StreamId>,
{
    pub fn new(domain_event: D) -> Self {
        let state = WithDomain {
            stream_id: domain_event.aggregate_id().clone().into(),
            event_type: domain_event.name().to_owned(),
            domain_event,
        };
        EventRecordBuilder { state }
    }

    pub fn with_version(self, version: u64) -> EventRecordBuilder<WithVersion<D>> {
        let state = WithVersion {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            event_type: self.state.event_type,
            version,
        };

        EventRecordBuilder { state }
    }
}

impl<D> EventRecordBuilder<WithVersion<D>>
where
    D: DomainEvent,
{
    pub fn with_metadata(self, metadata: EventMetadata) -> EventRecordBuilder<WithMetadata<D>> {
        let state = WithMetadata {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            event_type: self.state.event_type,
            metadata,
        };

        EventRecordBuilder { state }
    }
}

impl<D> EventRecordBuilder<WithMetadata<D>>
where
    D: DomainEvent,
{
    pub async fn serialize<F, Fut>(self, serializer: F) -> Result<EventRecord, Error>
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
        Ok(EventRecord::new(
            stream_id, event_type, version, metadata, payload,
        ))
    }
}

// type state pattern
// should it be pub(crate?)
//
// should any of these things be pub (crate)??
pub trait EventBuilderState {}

pub struct WithDomain<D>
where
    D: DomainEvent,
{
    stream_id: StreamId,
    event_type: String,
    domain_event: D,
}

impl<D> EventBuilderState for WithDomain<D> where D: DomainEvent {}

pub struct WithVersion<D>
where
    D: DomainEvent,
{
    version: u64,
    stream_id: StreamId,
    event_type: String,
    domain_event: D,
}

impl<D> EventBuilderState for WithVersion<D> where D: DomainEvent {}

pub struct WithMetadata<D>
where
    D: DomainEvent,
{
    stream_id: StreamId,
    domain_event: D,
    event_type: String,
    version: u64,
    metadata: EventMetadata,
}

impl<D> EventBuilderState for WithMetadata<D> where D: DomainEvent {}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn should_be_able_to_build_event_record_from_domain_events() {}

    #[tokio::test]
    async fn should_fail_to_build_if_serialization_fails() {}

    #[tokio::test]
    async fn should_be_able_to_build_event_record_with_metadata() {}

    #[tokio::test]
    async fn should_be_able_to_build_event_record_without_metadata() {}
}
