#![allow(dead_code)]
use super::{EventDeserializer, EventSerializer};
use crate::{DomainEvent, Id, error::Error};
use serde::{Deserialize, Serialize};
use std::default::Default;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecordId(Uuid);

impl Default for EventRecordId {
    fn default() -> Self {
        EventRecordId(Uuid::now_v7())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecord<I: Id> {
    id: EventRecordId,
    stream_id: I,
    version: u64,
    event_type: String,
    payload: Vec<u8>,
}

impl<I> EventRecord<I>
where
    I: Id,
{
    // do not want people to directly create EventRecord
    pub(crate) fn new(stream_id: I, event_type: String, version: u64, payload: Vec<u8>) -> Self {
        EventRecord {
            id: EventRecordId::default(),
            event_type,
            stream_id,
            version,
            payload,
        }
    }

    pub async fn event<E, De>(&self, deserializer: De) -> Result<E, Error>
    where
        De: EventDeserializer,
        E: DomainEvent<Id = I>,
    {
        deserializer.deserialize(&self.payload).await
    }

    pub fn stream_id(&self) -> &I {
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

    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<WithDomain<I, D>>
    where
        D: DomainEvent<Id = I>,
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

impl<D, I> EventRecordBuilder<WithDomain<I, D>>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub fn new(domain_event: D) -> Self {
        let state = WithDomain {
            stream_id: domain_event.aggregate_id().clone(),
            domain_event,
        };
        EventRecordBuilder { state }
    }

    pub fn with_version(self, version: u64) -> EventRecordBuilder<WithVersion<I, D>> {
        let state = WithVersion {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version,
        };

        EventRecordBuilder { state }
    }
}

impl<D, I> EventRecordBuilder<WithVersion<I, D>>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub fn with_event_type(self, event_type: &str) -> EventRecordBuilder<WithEventType<I, D>> {
        let state = WithEventType {
            stream_id: self.state.stream_id,
            domain_event: self.state.domain_event,
            version: self.state.version,
            event_type: event_type.to_string(),
        };

        EventRecordBuilder { state }
    }
}

impl<D, I> EventRecordBuilder<WithEventType<I, D>>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub async fn build<S, Fut>(self, serializer: &S) -> Result<EventRecord<I>, Error>
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

pub struct WithDomain<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    stream_id: I,
    domain_event: D,
}

impl<I, D> EventBuilderState for WithDomain<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
}

pub struct WithVersion<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    version: u64,
    stream_id: I,
    domain_event: D,
}

impl<I, D> EventBuilderState for WithVersion<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
}

pub struct WithEventType<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    event_type: String,
    stream_id: I,
    domain_event: D,
    version: u64,
}

impl<I, D> EventBuilderState for WithEventType<I, D>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn should_be_able_to_build_event_record_from_domain_events() {}

    #[tokio::test]
    async fn should_fail_to_build_if_serialization_fails() {}
}
