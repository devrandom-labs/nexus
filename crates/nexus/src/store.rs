use crate::{
    core::Id,
    error::Error,
    event::{PendingEvent, PersistedEvent},
};
use async_trait::async_trait;
use std::pin::Pin;
use tokio_stream::Stream;

#[async_trait]
pub trait EventStore {
    async fn append_to_stream<I>(
        &self,
        stream_id: &I,
        expected_version: u64,
        event_records: Vec<PendingEvent<I>>,
    ) -> Result<(), Error>
    where
        I: Id;

    fn read_stream<'a, I>(
        &'a self,
        stream_id: I,
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
        I: Id;
}
