use super::event_record::EventRecord;
use crate::{Id, error::Error};
use async_trait::async_trait;
use std::pin::Pin;
use tokio_stream::Stream;

#[async_trait]
pub trait EventStore {
    async fn append_to_stream<I>(
        &self,
        stream_id: &I,
        expected_version: u64,
        event_records: Vec<EventRecord<I>>,
    ) -> Result<(), Error>
    where
        I: Id;

    fn read_stream<I, 'a>(
        &'a self,
        stream_id: &I,
    ) -> Pin<Box<dyn Stream<Item = Result<EventRecord<I>, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
        I: Id;
}
