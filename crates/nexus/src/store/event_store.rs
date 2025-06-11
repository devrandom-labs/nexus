use super::event_record::EventRecord;
use crate::Id;
use std::{future::Future, pin::Pin};
use tower::BoxError;

pub trait EventStore {
    #[allow(clippy::type_complexity)]
    fn append_events<E, I>(
        event_records: E,
    ) -> Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'static>>
    where
        E: IntoIterator<Item = EventRecord<I>>,
        I: Id;

    #[allow(clippy::type_complexity)]
    fn fetch_events<I>(
        stream_id: I,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<EventRecord<I>>, BoxError>> + Send + Sync + 'static>>
    where
        I: Id;
}
