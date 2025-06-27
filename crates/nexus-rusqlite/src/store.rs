use async_trait::async_trait;
use nexus::{
    error::Error,
    store::{EventRecord, EventStore, StreamId},
};
use rusqlite::{Connection, Error as SqlError, config::DbConfig, params};
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
        event_records: Vec<EventRecord>,
    ) -> Result<(), Error> {
        debug!(?stream_id, expected_version, "appending events");
        let conn = Arc::clone(&self.connection);
        let mut conn = conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|err| Error::Store { source: err.into() })?;

        {
            let mut event_stmt = tx
                .prepare_cached("INSERT INTO event (id, stream_id, version, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)")
                .map_err(|err| Error::Store { source: err.into() })?;

            let mut event_metadata_stmt = tx
                .prepare_cached(
                    "INSERT INTO event_metadata (event_id, correlation_id) VALUES (?1, ?2)",
                )
                .map_err(|err| Error::Store { source: err.into() })?;

            for record in &event_records {
                event_stmt
                    .execute(params![
                        record.id().to_string(),
                        record.stream_id().to_string(),
                        record.version(),
                        record.event_type(),
                        record.payload()
                    ])
                    .map_err(|err| Error::Store { source: err.into() })?;

                event_metadata_stmt
                    .execute(params![
                        record.id().to_string(),
                        record.metadata().correlation_id().to_string()
                    ])
                    .map_err(|err| Error::Store { source: err.into() })?;
            }
        }

        tx.commit()
            .map_err(|err| Error::Store { source: err.into() })?;

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
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
