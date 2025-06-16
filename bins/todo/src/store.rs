use async_trait::async_trait;
use nexus::{
    error::Error,
    store::{
        EventStore,
        event_record::{EventRecord, StreamId},
    },
};
use rusqlite::{Connection, Error as SqlError};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio_stream::Stream;
use tracing::{debug, instrument};

#[derive(Debug)]
pub struct Store {
    #[allow(dead_code)]
    pub connection: Arc<Mutex<Connection>>,
}

impl Store {
    #[instrument]
    pub fn new() -> Result<Self, SqlError> {
        debug!("initializing store connection...");
        let connection = Connection::open_in_memory()?;
        let result = connection.execute(
            "CREATE TABLE IF NOT EXISTS nexus_events (
                 id TEXT PRIMARY KEY,
                 stream_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 event_type TEXT NOT NULL,
                 payload BLOB NOT NULL,
                 UNIQUE (stream_id, version)
            )",
            (),
        )?;
        debug!("nexus_events table created: {}", result);
        Ok(Store {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
}

#[async_trait]
impl EventStore for Store {
    #[instrument]
    async fn append_to_stream(
        &self,
        _stream_id: StreamId,
        _expected_version: u64,
        _event_records: Vec<EventRecord>,
    ) -> Result<(), Error> {
        todo!("insert into db");
    }

    #[instrument]
    fn read_stream<'a>(
        &'a self,
        _stream_id: StreamId,
    ) -> Pin<Box<dyn Stream<Item = Result<EventRecord, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
    {
        todo!("iterate and convert it to streams")
    }
}
