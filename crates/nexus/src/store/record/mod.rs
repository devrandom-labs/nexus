#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::{fmt::Display, ops::Deref, sync::Arc};

pub mod builder;
pub mod event_metadata;
pub mod event_record;

pub use event_record::{EventRecord, EventRecordResponse};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
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
