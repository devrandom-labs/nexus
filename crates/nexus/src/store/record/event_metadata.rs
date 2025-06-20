#![allow(dead_code)]
use super::CorrelationId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EventMetadata {
    correlation_id: CorrelationId,
}

impl EventMetadata {
    pub fn new(correlation_id: CorrelationId) -> Self {
        EventMetadata { correlation_id }
    }

    pub fn correlation_id(&self) -> &CorrelationId {
        &self.correlation_id
    }
}
