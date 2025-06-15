#![allow(dead_code)]
use super::{EventDeserializer, EventSerializer};
use crate::{DomainEvent, error::Error};
use serde::{Deserialize, Serialize};
use std::default::Default;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventRecordId(Uuid);

impl Default for EventRecordId {
    fn default() -> Self {
        EventRecordId(Uuid::now_v7())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamId(String);

impl StreamId {
    pub fn new(id: String) -> Self {
        StreamId(id)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecord {
    id: EventRecordId,
    stream_id: StreamId,
    version: u64,
    event_type: String,
    payload: Vec<u8>,
}

impl EventRecord {
    // do not want people to directly create EventRecord
    pub(crate) fn new<I>(stream_id: I, event_type: String, version: u64, payload: Vec<u8>) -> Self
    where
        I: Into<StreamId>,
    {
        EventRecord {
            id: EventRecordId::default(),
            event_type,
            stream_id: stream_id.into(),
            version,
            payload,
        }
    }

    pub async fn event<E, De>(&self, deserializer: De) -> Result<E, Error>
    where
        De: EventDeserializer,
        E: DomainEvent,
    {
        deserializer.deserialize(&self.payload).await
    }

    pub fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }

    pub fn id(&self) -> &EventRecordId {
        &self.id
    }

    pub fn version(&self) -> &u64 {
        &self.version
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<WithDomain<D>>
    where
        D: DomainEvent,
        D::Id: Into<StreamId>,
    {
        EventRecordBuilder::new(domain_event)
    }
}

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
    pub fn with_event_type(self, event_type: &str) -> EventRecordBuilder<WithEventType<D>> {
        let state = WithEventType {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            event_type: event_type.to_string(),
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
        } = self.state;
        let payload = serializer.serialize(domain_event).await?;
        Ok(EventRecord::new(stream_id, event_type, version, payload))
    }
}

// type state pattern
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

pub struct WithEventType<D>
where
    D: DomainEvent,
{
    event_type: String,
    stream_id: StreamId,
    domain_event: D,
    version: u64,
}

impl<D> EventBuilderState for WithEventType<D> where D: DomainEvent {}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn should_be_able_to_build_event_record_from_domain_events() {}

    #[tokio::test]
    async fn should_fail_to_build_if_serialization_fails() {}
}
