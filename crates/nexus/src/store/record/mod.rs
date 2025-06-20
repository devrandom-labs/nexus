#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::default::Default;
use uuid::Uuid;

pub mod builder;
pub mod event_metadata;
pub mod event_record;

pub use event_record::EventRecord;

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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CorrelationId(Uuid);

impl Default for CorrelationId {
    fn default() -> Self {
        CorrelationId(Uuid::new_v4())
    }
}
