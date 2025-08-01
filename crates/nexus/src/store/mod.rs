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

    fn read_stream<'a, I>(
        &'a self,
        stream_id: I,
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>>> + Send + 'a>>
    where
        Self: Sync + 'a,
        I: Id;
}
