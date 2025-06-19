use super::builder::{EventRecordBuilder, WithDomain};
use super::{EventRecordId, StreamId};
use crate::{DomainEvent, core::EventDeserializer, error::Error};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, default::Default};

#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecord {
    id: EventRecordId,
    stream_id: StreamId,
    version: u64,
    event_type: String,
    metadata: HashMap<String, String>,
    payload: Vec<u8>,
}

impl EventRecord {
    // do not want people to directly create EventRecord
    pub(crate) fn new<I>(
        stream_id: I,
        event_type: String,
        version: u64,
        metadata: HashMap<String, String>,
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

    pub async fn event<E, De>(&self, deserializer: De) -> Result<E, Error>
    where
        De: EventDeserializer,
        E: DomainEvent,
    {
        deserializer.deserialize(&self.payload).await
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

    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn builder<D>(domain_event: D) -> EventRecordBuilder<WithDomain<D>>
    where
        D: DomainEvent,
        D::Id: Into<StreamId>,
    {
        EventRecordBuilder::new(domain_event)
    }
}
