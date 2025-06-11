#![allow(dead_code)]
use crate::{DomainEvent, Id};
use serde::{Deserialize, Serialize};
use std::default::Default;
use thiserror::Error;
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

#[derive(Error, Debug)]
#[error("PayloadSerializationError")]
pub struct PayloadSerializationError;

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
    pub fn build<S>(self, serializer: S) -> Result<EventRecord<I>, PayloadSerializationError>
    where
        S: Fn(D) -> Result<Vec<u8>, PayloadSerializationError>,
    {
        let EventRecordBuilder {
            stream_id,
            domain_event,
        } = self.initial_event_record;
        let payload = serializer(domain_event)?;
        Ok(EventRecord::new(stream_id, self.version, payload))
    }
}
