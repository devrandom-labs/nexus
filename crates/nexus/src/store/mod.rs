use crate::{
    domain::Id,
    error::Result,
    event::{PendingEvent, PersistedEvent},
};
use async_trait::async_trait;
use std::pin::Pin;
use tokio_stream::Stream;

#[async_trait]
pub trait EventStore {
    async fn append_to_stream<I>(
        &self,
        expected_version: u64,
        pending_events: Vec<PendingEvent<I>>,
    ) -> Result<()>
    where
        I: Id;

    fn read_stream_from<I>(
        &self,
        stream_id: I,
        version: u64,
        size: u32,
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>>> + Send>>
    where
        Self: Sync,
        I: Id;
}

pub trait EventStreamer {
    fn read_stream_from_checkpoint<I>(
        &self,
        stream_name: &str,
        version: u64,
        size: u32,
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>>> + Send>>
    where
        Self: Sync,
        I: Id;
}

// TODO: added event stream to pending events
// TODO: each aggregate should have a name / type (useful for projections)
