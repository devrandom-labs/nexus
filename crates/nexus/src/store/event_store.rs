use super::event_record::EventRecord;
use crate::{Id, error::Error};
use async_trait::async_trait;

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

    async fn read_stream<I>(&self, stream_id: &I) -> Result<Vec<EventRecord<I>>, Error>
    where
        I: Id;
}
