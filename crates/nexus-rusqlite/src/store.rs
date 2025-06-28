use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nexus::{
    error::Error,
    store::{
        EventRecord, EventStore, StreamId,
        record::{CorrelationId, EventRecordId, EventRecordResponse},
    },
};
use rusqlite::{Connection, Error as SqlError, Result as SResult, config::DbConfig, params};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{sync::mpsc::channel, task::spawn_blocking};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tracing::{debug, instrument, trace};
use uuid::Uuid;

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

    fn read_stream<'a>(
        &'a self,
        stream_id: StreamId,
    ) -> Pin<Box<dyn Stream<Item = Result<EventRecordResponse, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
    {
        debug!(?stream_id, "fetching events");
        let (tx, mut rx) = channel::<Result<EventRecordResponse, Error>>(10);
        let conn = Arc::clone(&self.connection);
        let sql = "
            SELECT e.id, e.stream_id, e.version, e.event_type, e.payload, e.persisted_at, m.correlation_id
            FROM event AS e
            INNER JOIN event_metadata AS m ON e.id = m.event_id
            WHERE e.stream_id = ?1
            ORDER BY e.version ASC
        ";
        let task = spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt_result = conn
                .prepare(sql)
                .map_err(|err| Error::Store { source: err.into() });

            if let Ok(mut stmt) = stmt_result {
                let query_result = stmt
                    .query_and_then([&stream_id.to_string()], |row| {
                        // correlation
                        let correlation_id: SResult<String> = row.get("correlation_id");
                        let id: SResult<Uuid> = row.get("id");
                        let stream_id: SResult<String> = row.get("stream_id");
                        let version: SResult<u64> = row.get("version");
                        let event_type: SResult<String> = row.get("version");
                        let payload: SResult<Vec<u8>> = row.get("payload");
                        let persisted_at: SResult<DateTime<Utc>> = row.get("persisted_at");

                        todo!()
                    })
                    .map_err(|err| Error::Store { source: err.into() });

                if let Err(err) = query_result {
                    tx.send(Err(err));
                }
            } else {
                tx.send(Err(stmt_result.unwrap_err()));
            }
        });

        let stream: Pin<Box<dyn Stream<Item = Result<EventRecordResponse, Error>> + Send + 'a>> =
            Box::pin(ReceiverStream::new(rx));
        stream
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn should_have_unique_contraint_on_stream_id_and_version() {}
    #[test]
    fn should_have_foreign_key_config_on() {}
}

// TODO: performance test it
// // TODO: property test it
//
// // TODO: property test it
// TODO: feature for inmemory
// TODO: feature for tracing
