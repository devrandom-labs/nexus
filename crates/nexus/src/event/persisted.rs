use super::metadata::EventMetadata;
use crate::{domain::Id, infra::EventId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedEvent<I>
where
    I: Id,
{
    pub id: EventId,
    pub stream_id: I,
    pub version: u64,
    pub event_type: String,
    pub metadata: EventMetadata,
    pub payload: Vec<u8>,
    pub persisted_at: DateTime<Utc>,
}

impl<I> PersistedEvent<I>
where
    I: Id,
{
    // do not want people to directly create EventRecord
    pub fn new(
        id: EventId,
        stream_id: I,
        event_type: String,
        version: u64,
        metadata: EventMetadata,
        payload: Vec<u8>,
        persisted_at: DateTime<Utc>,
    ) -> Self {
        PersistedEvent {
            id,
            event_type,
            stream_id,
            metadata,
            version,
            payload,
            persisted_at,
        }
    }
}
