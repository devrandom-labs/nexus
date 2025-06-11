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
    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<I>
    where
        D: DomainEvent<Id = I>,
    {
        EventRecordBuilder::new(domain_event)
    }
}

#[derive(Debug, Error)]
enum EventRecordBuilderError {
    #[error("")]
    SerializingError,
}

pub struct EventRecordBuilder<D, I>
where
    I: Id,
    D: DomainEvent<Id = I>,
{
    domain_event: D,
}

impl<I> EventRecordBuilder<I>
where
    I: Id,
{
    pub fn new<D>(domain_event: D) -> Result<Self, EventRecordBuilderError>
    where
        D: DomainEvent<Id = I>,
    {
        todo!()
    }
}
