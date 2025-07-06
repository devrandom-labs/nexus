use super::record::{EventRecord, EventRecordResponse, StreamId};
use crate::error::Error;
use async_trait::async_trait;
use std::pin::Pin;
use tokio_stream::Stream;

#[async_trait]
pub trait EventStore {
    async fn append_to_stream(
        &self,
        stream_id: &StreamId,
        expected_version: u64,
        event_records: Vec<EventRecord>,
    ) -> Result<(), Error>;

    fn read_stream<'a>(
        &'a self,
        stream_id: StreamId,
    ) -> Pin<Box<dyn Stream<Item = Result<EventRecordResponse, Error>> + Send + 'a>>
    where
        Self: Sync + 'a;
}
