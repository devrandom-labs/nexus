use super::event_record::EventRecord;
use crate::Id;
use std::{future::Future, pin::Pin};
use tower::BoxError;

pub trait EventStore {
    #[allow(clippy::type_complexity)]
    fn append_events<I>(
        &self,
        stream_id: &I,
        expected_version: u64, // The crucial parameter for the concurrency check
        event_records: Vec<EventRecord<I>>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'static>>
    where
        I: Id;

    #[allow(clippy::type_complexity)]
    fn fetch_events<I>(
        &self,
        stream_id: &I,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<EventRecord<I>>, BoxError>> + Send + Sync + 'static>>
    where
        I: Id;
}
