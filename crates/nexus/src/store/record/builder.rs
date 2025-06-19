#![allow(dead_code)]
use super::{StreamId, event_record::EventRecord};
use crate::{DomainEvent, core::EventSerializer, error::Error};
use std::collections::HashMap;

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
            domain_event,
        };
        EventRecordBuilder { state }
    }

    pub fn with_version(self, version: u64) -> EventRecordBuilder<WithVersion<D>> {
        let state = WithVersion {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version,
        };

        EventRecordBuilder { state }
    }
}

impl<D> EventRecordBuilder<WithVersion<D>>
where
    D: DomainEvent,
{
    pub fn with_metadata(
        self,
        metadata: HashMap<String, String>,
    ) -> EventRecordBuilder<WithMetadata<D>> {
        let state = WithMetadata {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            metadata,
        };

        EventRecordBuilder { state }
    }

    pub fn with_event_type<E>(self, event_type: E) -> EventRecordBuilder<WithEventType<D>>
    where
        E: Into<String>,
    {
        let state = WithEventType {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            metadata: HashMap::new(),
            event_type: event_type.into(),
        };

        EventRecordBuilder { state }
    }
}

impl<D> EventRecordBuilder<WithMetadata<D>>
where
    D: DomainEvent,
{
    pub fn with_event_type<E>(self, event_type: E) -> EventRecordBuilder<WithEventType<D>>
    where
        E: Into<String>,
    {
        let state = WithEventType {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            metadata: self.state.metadata,
            event_type: event_type.into(),
        };
        EventRecordBuilder { state }
    }
}

impl<D> EventRecordBuilder<WithEventType<D>>
where
    D: DomainEvent,
{
    pub async fn build<S, Fut>(self, serializer: &S) -> Result<EventRecord, Error>
    where
        S: EventSerializer,
    {
        let WithEventType {
            stream_id,
            domain_event,
            version,
            event_type,
            metadata,
        } = self.state;
        let payload = serializer.serialize(domain_event).await?;
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
    domain_event: D,
}

impl<D> EventBuilderState for WithDomain<D> where D: DomainEvent {}

pub struct WithVersion<D>
where
    D: DomainEvent,
{
    version: u64,
    stream_id: StreamId,
    domain_event: D,
}

impl<D> EventBuilderState for WithVersion<D> where D: DomainEvent {}

pub struct WithMetadata<D>
where
    D: DomainEvent,
{
    stream_id: StreamId,
    domain_event: D,
    version: u64,
    metadata: HashMap<String, String>,
}

impl<D> EventBuilderState for WithMetadata<D> where D: DomainEvent {}

pub struct WithEventType<D>
where
    D: DomainEvent,
{
    event_type: String,
    stream_id: StreamId,
    domain_event: D,
    version: u64,
    metadata: HashMap<String, String>,
}

impl<D> EventBuilderState for WithEventType<D> where D: DomainEvent {}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn should_be_able_to_build_event_record_from_domain_events() {}

    #[tokio::test]
    async fn should_fail_to_build_if_serialization_fails() {}

    #[tokio::test]
    async fn should_be_able_to_build_event_record_with_metadata() {}

    #[tokio::test]
    async fn shoudl_be_able_to_build_event_record_without_metadata() {}
}
