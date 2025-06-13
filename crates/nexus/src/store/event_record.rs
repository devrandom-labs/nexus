#![allow(dead_code)]
use super::{EventDeserializer, EventSerializer};
use crate::{DomainEvent, Id};
use serde::{Deserialize, Serialize};
use std::default::Default;
use tower::BoxError;
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
    payload: Vec<u8>,
}

impl<I> EventRecord<I>
where
    I: Id,
{
    // do not want people to directly create EventRecord
    pub(crate) fn new(stream_id: I, version: u64, payload: Vec<u8>) -> Self {
        EventRecord {
            id: EventRecordId::default(),
            stream_id,
            version,
            payload,
        }
    }

    pub async fn event<E, De>(&self, deserializer: De) -> Result<E, BoxError>
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

    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<D, I>
    where
        D: DomainEvent<Id = I>,
    {
        EventRecordBuilder::new(domain_event)
    }
}

pub struct EventRecordBuilder<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    stream_id: I,
    domain_event: D,
}

impl<D, I> EventRecordBuilder<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub fn new(domain_event: D) -> Self {
        EventRecordBuilder {
            stream_id: domain_event.aggregate_id().clone(),
            domain_event,
        }
    }

    pub fn with_version(self, version: u64) -> EventRecordBuilderWithVersion<D, I> {
        let EventRecordBuilder {
            stream_id,
            domain_event,
        } = self;
        EventRecordBuilderWithVersion {
            stream_id,
            domain_event,
            version,
        }
    }
}

pub struct EventRecordBuilderWithVersion<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    stream_id: I,
    domain_event: D,
    version: u64,
}

impl<D, I> EventRecordBuilderWithVersion<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    pub async fn build<S, Fut>(self, serializer: &S) -> Result<EventRecord<I>, BoxError>
    where
        S: EventSerializer,
    {
        let EventRecordBuilderWithVersion {
            stream_id,
            domain_event,
            version,
        } = self;
        let payload = serializer.serialize(domain_event).await?;
        Ok(EventRecord::new(stream_id, version, payload))
    }
}

#[cfg(test)]
mod test {

    #[tokio::test]
    async fn should_be_able_to_build_event_record_from_domain_events() {}

    #[tokio::test]
    async fn should_fail_to_build_if_serialization_fails() {}
}
