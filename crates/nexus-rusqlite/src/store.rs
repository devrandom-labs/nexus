use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nexus::{
    error::Error,
    store::{
        EventRecord, EventStore, StreamId,
        record::{
            CorrelationId, EventRecordId, EventRecordResponse, event_metadata::EventMetadata,
        },
    },
};
use rusqlite::{Connection, Result as SResult, Row, config::DbConfig, params};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc::channel, oneshot},
    task::spawn_blocking,
};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tracing::{debug, instrument, trace};
use uuid::Uuid;

// for any given stream_id, the stream of events I read back must be identical to content and order to the stream of events I wrote
//
// TODO: The Foundation (Classic Unit Test): Start with the simplest case.
// TODO: The Generalization (Property Test): Elevate the simple case to a universal law.
// TODO: The Chaos (Fuzz Test): Attack the boundaries with invalid and malicious data.
// TODO: The Structure (Snapshot Test): Ensure the physical data format remains stable.
// TODO: The Audit (Mutation Test): Test the quality of our other tests.
//
//
#[derive(Debug, Clone)]
pub struct Store {
    #[allow(dead_code)]
    pub connection: Arc<Mutex<Connection>>,
}

impl Store {
    #[instrument(level = "debug", err)]
    pub fn new() -> Result<Self, Error> {
        trace!("Opening SQLite connection in-memory.");
        let conn = Connection::open_in_memory()
            .map_err(|err| Error::ConnectionFailed { source: err.into() })?;
        debug!("Applying database connection configurations for security and integrity...");
        Self::configure_connection(&conn)
            .map_err(|err| Error::ConnectionFailed { source: err.into() })?;
        Ok(Store {
            connection: Arc::new(Mutex::new(conn)),
        })
    }

    #[instrument(level = "debug", skip(conn), err)]
    fn configure_connection(conn: &Connection) -> SResult<()> {
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, true)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false)?;
        Ok(())
    }

    #[inline]
    #[instrument(level = "debug", skip(row), err)]
    fn get_event_response(row: &Row<'_>) -> SResult<EventRecordResponse> {
        let correlation_id = row
            .get::<_, String>("correlation_id")
            .map(CorrelationId::new)?;
        let id = row.get::<_, Uuid>("id").map(Into::<EventRecordId>::into)?;
        let stream_id = row.get::<_, String>("stream_id").map(StreamId::new)?;
        let version = row.get::<_, u64>("version")?;
        let event_type = row.get::<_, String>("event_type")?;
        let payload = row.get::<_, Vec<u8>>("payload")?;
        let persisted_at = row.get::<_, DateTime<Utc>>("persisted_at")?;
        let metadata = EventMetadata::new(correlation_id);
        Ok(EventRecordResponse::new(
            id,
            stream_id,
            event_type,
            version,
            metadata,
            payload,
            persisted_at,
        ))
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

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let conn = Arc::clone(&self.connection);

        spawn_blocking(move || {
            let result = (|| {
                let mut conn = conn.lock().map_err(|_| Error::System {
                    reason: "mutex lock poisoned..".to_string(),
                })?;

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
            })();

            let _ = tx.send(result);
        });

        rx.await.map_err(|_| Error::System {
            reason: "Write stream for one shot channel panicked".to_string(),
        })?
    }

    fn read_stream<'a>(
        &'a self,
        stream_id: StreamId,
    ) -> Pin<Box<dyn Stream<Item = Result<EventRecordResponse, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
    {
        debug!(?stream_id, "fetching events");
        let (tx, rx) = channel::<Result<EventRecordResponse, Error>>(10);
        let conn = Arc::clone(&self.connection);
        spawn_blocking(move || {
            let result = (|| {
                let conn = conn.lock().map_err(|_| Error::System {
                    reason: "mutex lock poisoned..".to_string(),
                })?;
                let sql = "
                        SELECT e.id, e.stream_id, e.version, e.event_type, e.payload, e.persisted_at, m.correlation_id
                        FROM event AS e
                        INNER JOIN event_metadata AS m ON e.id = m.event_id
                        WHERE e.stream_id = ?1
                        ORDER BY e.version ASC
                    ";

                let mut stmt = conn
                    .prepare(sql)
                    .map_err(|err| Error::Store { source: err.into() })?;
                let mut rows = stmt
                    .query([&stream_id.to_string()])
                    .map_err(|err| Error::Store { source: err.into() })?;

                while let Some(row) = rows
                    .next()
                    .map_err(|err| Error::Store { source: err.into() })?
                {
                    let response = Self::get_event_response(row)
                        .map_err(|err| Error::Store { source: err.into() })?;

                    if tx.blocking_send(Ok(response)).is_err() {
                        break;
                    }
                }
                Ok(())
            })();

            if let Err(e) = result {
                let _ = tx.blocking_send(Err(e));
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

#[cfg(test)]
mod tests {
    use events::UserCreated;
    use nexus::store::{
        EventRecord,
        record::{CorrelationId, event_metadata::EventMetadata},
    };
    use refinery::embed_migrations;
    use rusqlite::Connection;

    embed_migrations!("migrations");

    fn apply_migrations() {
        let mut conn = Connection::open_in_memory().expect("could not open connection");
        migrations::runner()
            .run(&mut conn)
            .expect("migrations could not be applied.");
    }

    pub mod events {
        use nexus::DomainEvent;
        use serde::{Deserialize, Serialize};

        #[allow(dead_code)]
        #[derive(DomainEvent, Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct UserCreated {
            #[attribute_id]
            pub user_id: String,
        }
    }

    #[tokio::test]
    async fn should_be_able_to_write_and_read_stream_events() {
        apply_migrations();
        let domain_event = UserCreated {
            user_id: "1".to_string(),
        };
        let metadata = EventMetadata::new("1-corr".into());
        let record = EventRecord::builder(domain_event)
            .with_version(1)
            .with_metadata(metadata)
            .with_event_type("UserCreated".to_string()); // event type should just be struct name at this point and optional

        unimplemented!()
        // TODO: write to event store
        // TODO: read event record response
    }
}
