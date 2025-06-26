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

#[derive(Debug, Clone)]
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

    #[inline]
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
    #[instrument(level = "debug", skip(self), err)]
    async fn append_to_stream(
        &self,
        stream_id: StreamId,
        expected_version: u64,

        _event_records: Vec<EventRecord>,
    ) -> Result<(), Error> {
        debug!(?stream_id, expected_version, "appending events");
        let conn = &self.connection.lock().unwrap();
        let current_version = conn
            .query_row_and_then(
                "SELECT MAX(version) FROM event WHERE stream_id = ?1",
                [&stream_id.to_string()],
                |row| row.get::<_, u64>("version"),
            )
            .map_err(|err| Error::Store { source: err.into() })?;
        trace!(
            "stream_id: {}, current_version: {}",
            stream_id, current_version
        );
        if current_version != expected_version {
            return Err(Error::Conflict {
                stream_id,
                expected_version,
            });
        }
        todo!("insert into db");
    }

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

    #[test]
    fn should_have_unique_contraint_on_stream_id_and_version() {}
    #[test]
    fn should_have_foreign_key_config_on() {}
}
