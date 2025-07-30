use std::num::NonZeroU64;

use super::builder::{PendingEventBuilder, WithStreamId};
use super::metadata::EventMetadata;
use crate::{
    domain::Id,
    error::{Error, Result},
    infra::EventId,
};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "testing", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEvent<I>
where
    I: Id,
{
    id: EventId,
    stream_id: I,
    version: NonZeroU64,
    event_type: String,
    metadata: EventMetadata,
    payload: Vec<u8>,
}

impl<I> PendingEvent<I>
where
    I: Id,
{
    pub(crate) fn new(
        stream_id: I,
        version: NonZeroU64,
        event_type: String,
        metadata: EventMetadata,
        payload: Vec<u8>,
    ) -> Result<Self> {
        if event_type.trim().is_empty() {
            return Err(Error::InvalidArgument {
                name: "event_type".to_string(),
                reason: "must not be empty".to_string(),
                context: "PendingEvent::new".to_string(),
            });
        }

        Ok(PendingEvent {
            id: EventId::default(),
            stream_id,
            version,
            event_type,
            metadata,
            payload,
        })
    }

    pub fn builder(stream_id: I) -> PendingEventBuilder<WithStreamId<I>> {
        PendingEventBuilder::new(stream_id)
    }
}

impl<I> PendingEvent<I>
where
    I: Id,
{
    pub fn stream_id(&self) -> &I {
        &self.stream_id
    }

    pub fn id(&self) -> &EventId {
        &self.id
    }

    pub fn version(&self) -> &NonZeroU64 {
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
}
