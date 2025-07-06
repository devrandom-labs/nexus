use super::{
    EventRecordId, StreamId,
    builder::{EventRecordBuilder, WithDomain},
    event_metadata::EventMetadata,
};
use crate::{
    core::{DomainEvent, EventDeserializer},
    error::Error,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::default::Default;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventRecord {
    id: EventRecordId,
    stream_id: StreamId,
    version: u64,
    event_type: String,
    metadata: EventMetadata,
    payload: Vec<u8>,
}

impl EventRecord {
    // do not want people to directly create EventRecord
    pub(crate) fn new<I>(
        stream_id: I,
        event_type: String,
        version: u64,
        metadata: EventMetadata,
        payload: Vec<u8>,
    ) -> Self
    where
        I: Into<StreamId>,
    {
        EventRecord {
            id: EventRecordId::default(),
            event_type,
            stream_id: stream_id.into(),
            metadata,
            version,
            payload,
        }
    }

    pub fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }

    pub fn id(&self) -> &EventRecordId {
        &self.id
    }

    pub fn version(&self) -> &u64 {
        &self.version
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn metadata(&self) -> &EventMetadata {
        &self.metadata
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<WithDomain<D>>
    where
        D: DomainEvent,
        D::Id: Into<StreamId>,
    {
        EventRecordBuilder::new(domain_event)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecordResponse {
    pub id: EventRecordId,
    pub stream_id: StreamId,
    pub version: u64,
    pub event_type: String,
    pub metadata: EventMetadata,
    pub payload: Vec<u8>,
    pub persisted_at: DateTime<Utc>,
}

impl EventRecordResponse {
    // do not want people to directly create EventRecord
    pub fn new(
        id: EventRecordId,
        stream_id: StreamId,
        event_type: String,
        version: u64,
        metadata: EventMetadata,
        payload: Vec<u8>,
        persisted_at: DateTime<Utc>,
    ) -> Self {
        EventRecordResponse {
            id,
            event_type,
            stream_id,
            metadata,
            version,
            payload,
            persisted_at,
        }
    }

    pub async fn event<E, De>(&self, deserializer: De) -> Result<E, Error>
    where
        De: EventDeserializer,
        E: DomainEvent,
    {
        deserializer.deserialize(&self.payload).await
    }
}
