// use async_trait::async_trait;
// use nexus::store::{EventStore, event_record::EventRecord};

// pub struct Store;

// #[async_trait]
// impl EventStore for Store {
//     async fn append_events<I>(
//         &self,
//         stream_id: &I,
//         expected_version: u64,
//         event_records: Vec<EventRecord<I>>,
//     ) -> Result<(), BoxError>
//     where
//         I: Id,
//     {
//     }

//     async fn read_stream_forward<I>(&self, stream_id: &I) -> Result<Vec<EventRecord<I>>, BoxError>
//     where
//         I: Id,
//     {
//     }
// }
