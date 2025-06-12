#![allow(dead_code)]
use super::event_serializer::EventSerializer;
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
        EventRecordBuilderWithVersion {
            initial_event_record: self,
            version,
        }
    }
}

pub struct EventRecordBuilderWithVersion<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    initial_event_record: EventRecordBuilder<D, I>,
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
        let EventRecordBuilder {
            stream_id,
            domain_event,
        } = self.initial_event_record;
        let payload = serializer.serialize(domain_event).await?;
        Ok(EventRecord::new(stream_id, self.version, payload))
    }
}
