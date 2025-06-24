use async_trait::async_trait;
use nexus::{
    error::Error,
    store::{EventRecord, EventStore, StreamId},
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
        stream_id: StreamId,
        expected_version: u64,
        _event_records: Vec<EventRecord>,
    ) -> Result<(), Error> {
        debug!(?stream_id, expected_version, "appending events");
        // TODO: get latest version for a stream_id
        // TODO: if version does not match return Error::Conflict or something?
        // TODO: write the events to the db
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

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, Result};
    use std::collections::HashSet;

    mod embedded {
        use refinery::embed_migrations;
        embed_migrations!("migrations");
    }

    fn return_table_fields(table_name: &str, conn: &mut Connection) -> Result<HashSet<String>> {
        let mut stm = conn
            .prepare(&format!("PRAGMA table_info('{}')", table_name))
            .unwrap();

        stm.query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<HashSet<String>>>()
    }

    #[test]
    fn should_have_events_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        embedded::migrations::runner().run(&mut conn).unwrap();
        let actual_fields = return_table_fields("event", &mut conn).unwrap();
        assert!(!actual_fields.is_empty(), "migration has not been applied.");
        for &expected_field in &[
            "id",
            "stream_id",
            "version",
            "event_type",
            "payload",
            "persisted_at",
        ] {
            assert!(actual_fields.contains(expected_field));
        }
    }

    #[test]
    fn should_have_event_metadata_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        embedded::migrations::runner().run(&mut conn).unwrap();
        let actual_fields = return_table_fields("event_metadata", &mut conn).unwrap();
        assert!(!actual_fields.is_empty(), "migration has not been applied.");
        for &expected_field in &["event_id", "correlation_id"] {
            assert!(actual_fields.contains(expected_field));
        }
    }
    #[test]
    fn should_have_unique_contraint_on_stream_id_and_version() {}
    #[test]
    fn should_have_foreign_key_config_on() {}
}
