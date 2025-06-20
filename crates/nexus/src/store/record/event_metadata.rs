#![allow(dead_code)]
use super::CorrelationId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct EventMetadata {
    correlation_id: CorrelationId,
}

impl EventMetadata {
    pub fn correlation_id(&self) -> &CorrelationId {
        &self.correlation_id
    }
}
