use super::event_record::EventRecord;
use crate::Id;
use async_trait::async_trait;
use tower::BoxError;

#[async_trait]
pub trait EventStore {
    async fn append_events<I>(
        &self,
        stream_id: &I,
        expected_version: u64,
        event_records: Vec<EventRecord<I>>,
    ) -> Result<(), BoxError>
    where
        I: Id;

    async fn read_stream_forward<I>(&self, stream_id: &I) -> Result<Vec<EventRecord<I>>, BoxError>
    where
        I: Id;
}
