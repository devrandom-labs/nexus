use async_trait::async_trait;
use nexus::{
    error::Error,
    store::{EventRecord, EventStore, StreamId},
};
use rusqlite::{Connection, Error as SqlError, config::DbConfig};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio_stream::Stream;
use tracing::{debug, instrument, trace};

#[derive(Debug)]
pub struct Store {
    #[allow(dead_code)]
    pub connection: Arc<Mutex<Connection>>,
}

impl Store {
    #[instrument(level = "debug", err)]
    pub fn new() -> Result<Self, SqlError> {
        trace!("Opening SQLite connection in-memory.");
        let conn = Connection::open_in_memory()?;
        debug!("Applying database connection configurations for security and integrity...");
        Self::configure_connection(&conn)?;
        Ok(Store {
            connection: Arc::new(Mutex::new(conn)),
        })
    }

    #[instrument(level = "debug", skip(conn), err)]
    fn configure_connection(conn: &Connection) -> Result<(), SqlError> {
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, true)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false)?;
        Ok(())
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

    mod embedded {
        use refinery::embed_migrations;
        embed_migrations!("migrations");
    }

    #[test]
    fn should_have_unique_contraint_on_stream_id_and_version() {}
    #[test]
    fn should_have_foreign_key_config_on() {}
}
