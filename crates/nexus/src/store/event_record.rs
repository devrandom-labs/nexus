#![allow(dead_code)]
use crate::Id;
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
    payload: Vec<u8>,
}

impl<I> EventRecord<I>
where
    I: Id,
{
    pub fn new(stream_id: I, payload: Vec<u8>) -> Self {
        EventRecord {
            id: EventRecordId::default(),
            stream_id,
            version: 0,
            payload,
        }
    }
}
