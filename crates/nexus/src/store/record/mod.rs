#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::{default::Default, fmt::Display, ops::Deref, sync::Arc};
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

impl Display for EventRecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for EventRecordId {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamId(Arc<String>);

impl StreamId {
    pub fn new(id: String) -> Self {
        StreamId(Arc::new(id))
    }
}

impl Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for StreamId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for StreamId {
    fn from(id: String) -> Self {
        Self(Arc::new(id))
    }
}

impl From<&str> for StreamId {
    fn from(id: &str) -> Self {
        Self(Arc::new(id.to_string()))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CorrelationId(pub Arc<String>);

impl CorrelationId {
    pub fn new(id: String) -> Self {
        CorrelationId(Arc::new(id))
    }
}

impl Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for CorrelationId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for CorrelationId {
    fn from(id: String) -> Self {
        Self(Arc::new(id))
    }
}

impl From<&str> for CorrelationId {
    fn from(id: &str) -> Self {
        Self(Arc::new(id.to_string()))
    }
}
