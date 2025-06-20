#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::{default::Default, sync::Arc};
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

// TODO: update this to have ARC inside
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamId(Arc<String>);

impl StreamId {
    pub fn new(id: String) -> Self {
        StreamId(Arc::new(id))
    }
}

// TODO: update these to have arc inside, so it is not cloned
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CorrelationId(pub Arc<String>);

impl CorrelationId {
    pub fn new(id: String) -> Self {
        CorrelationId(Arc::new(id))
    }
}
