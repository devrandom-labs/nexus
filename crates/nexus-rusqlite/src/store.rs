use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nexus::{
    domain::Id,
    error::Error,
    event::{EventMetadata, PendingEvent, PersistedEvent},
    infra::{CorrelationId, EventId},
    store::EventStore,
};
use rusqlite::{Connection, Result as SResult, Row, config::DbConfig, ffi, params};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc::channel, oneshot},
    task::spawn_blocking,
};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tracing::{debug, instrument};
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
    pub fn new(conn: Connection) -> Result<Self, Error> {
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
    fn get_event_response<I>(row: &Row<'_>) -> SResult<PersistedEvent<I>>
    where
        I: Id,
    {
        let correlation_id = row
            .get::<_, String>("correlation_id")
            .map(CorrelationId::new)?;
        let id = row.get::<_, Uuid>("id").map(Into::<EventId>::into)?;

        let stream_id_uuid = row.get::<_, Uuid>("stream_id")?;
        let stream_id = I::from_str(&stream_id_uuid.to_string()).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                "Failed to parse Id from string".into(),
            )
        })?;

        let version = row.get::<_, u64>("version")?;
        let event_type = row.get::<_, String>("event_type")?;
        let payload = row.get::<_, Vec<u8>>("payload")?;
        let persisted_at = row.get::<_, DateTime<Utc>>("persisted_at")?;
        let metadata = EventMetadata::new(correlation_id);
        Ok(PersistedEvent::new(
            id,
            stream_id,
            event_type,
            version,
            metadata,
            payload,
            persisted_at,
        ))
    }

    fn convert_sqlite_error(
        error: rusqlite::Error,
        stream_id: &impl ToString,
        event_id: &impl ToString,
        expected_version: u64,
    ) -> Error {
        if let rusqlite::Error::SqliteFailure(e, _) = &error {
            return match e.extended_code {
                ffi::SQLITE_CONSTRAINT_UNIQUE => Error::Conflict {
                    stream_id: stream_id.to_string(),
                    expected_version,
                },
                ffi::SQLITE_CONSTRAINT_PRIMARYKEY => Error::UniqueIdViolation {
                    id: event_id.to_string(),
                },
                _ => Error::Store {
                    source: error.into(),
                },
            };
        }

        Error::Store {
            source: error.into(),
        }
    }
}

#[async_trait]
impl EventStore for Store {
    #[instrument(level = "debug", skip(self), err)]
    async fn append_to_stream<I>(
        &self,
        stream_id: &I,
        expected_version: u64,
        event_records: Vec<PendingEvent<I>>,
    ) -> Result<(), Error>
    where
        I: Id,
    {
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
                                record.id().as_uuid(),
                                record.stream_id().as_ref(),
                                record.version(),
                                record.event_type(),
                                record.payload()
                            ])
                            .map_err(|err| {
                                Self::convert_sqlite_error(
                                    err,
                                    record.stream_id(),
                                    record.id(),
                                    expected_version,
                                )
                            })?;

                        event_metadata_stmt
                            .execute(params![
                                record.id().as_uuid(),
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

    fn read_stream<'a, I>(
        &'a self,
        stream_id: I,
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>, Error>> + Send + 'a>>
    where
        Self: Sync + 'a,
        I: Id,
    {
        debug!(?stream_id, "fetching events");
        let (tx, rx) = channel::<Result<PersistedEvent<I>, Error>>(10);
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
                    .query([&stream_id.as_ref()])
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
    use chrono::Utc;
    use events::UserCreated;
    use futures::TryStreamExt;
    use nexus::{
        error::Error,
        event::{EventMetadata, PendingEvent},
        infra::NexusId,
        store::EventStore,
    };
    use refinery::embed_migrations;
    use rusqlite::Connection;
    use serde_json::to_vec;

    use super::Store;

    embed_migrations!("migrations");

    fn apply_migrations() -> Connection {
        let mut conn = Connection::open_in_memory().expect("could not open connection");
        migrations::runner()
            .run(&mut conn)
            .expect("migrations could not be applied.");
        conn
    }

    pub mod events {
        use nexus::{DomainEvent, infra::NexusId};
        use serde::{Deserialize, Serialize};

        #[derive(DomainEvent, Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[domain_event(name = "user_created_v1")]
        pub struct UserCreated {
            pub user_id: NexusId,
        }
    }

    #[tokio::test]
    async fn should_be_able_to_write_and_read_stream_events() {
        let conn = apply_migrations();
        let store = Store::new(conn).expect("Store should be initialized");
        let id = NexusId::default();
        let domain_event = UserCreated { user_id: id };
        let metadata = EventMetadata::new("1-corr".into());

        let stream_id = NexusId::default();
        // its now a builder, figure a good fluent way that makes sense.

        let pending_event = PendingEvent::builder(stream_id)
            .with_version(1)
            .with_metadata(metadata)
            .with_domain(domain_event)
            .build(|domain_event| async move {
                to_vec(&domain_event)
                    .map_err(|err| Error::SerializationError { source: err.into() })
            })
            .await;

        assert!(
            pending_event.is_ok(),
            "pending event should be deserialized"
        );
        let record = pending_event.unwrap();
        store
            .append_to_stream(&stream_id, 1, vec![record.clone()])
            .await
            .unwrap();

        let events = store
            .read_stream(stream_id)
            .try_collect::<Vec<_>>()
            .await
            .expect("Read stream should succeed");

        assert_eq!(events.len(), 1, "there should be 1 event to read");
        let read_event = &events[0];

        assert_eq!(record.id(), &read_event.id, "event id should match");
        assert_eq!(
            record.stream_id(),
            &read_event.stream_id,
            "stream id should match"
        );
        assert_eq!(
            record.version(),
            &read_event.version,
            "version should match"
        );
        assert_eq!(
            record.event_type(),
            &read_event.event_type,
            "type should match"
        );
        assert_eq!(
            record.payload(),
            &read_event.payload,
            "payload should match"
        );
        assert_eq!(
            record.metadata().correlation_id(),
            read_event.metadata.correlation_id(),
            "correlation id should match"
        );
        assert!(read_event.persisted_at > (Utc::now() - chrono::Duration::seconds(5)));
    }

    #[tokio::test]
    async fn should_not_append_event_if_version_is_same() {
        let conn = apply_migrations();
        let store = Store::new(conn).expect("Store should be initialized");
        let id = NexusId::default();
        let domain_event = UserCreated { user_id: id };
        let metadata = EventMetadata::new("1-corr".into());

        let stream_id = NexusId::default();
        // its now a builder, figure a good fluent way that makes sense.

        let pending_event_1 = PendingEvent::builder(stream_id)
            .with_version(1)
            .with_metadata(metadata.clone())
            .with_domain(domain_event.clone())
            .build(|domain_event| async move {
                to_vec(&domain_event)
                    .map_err(|err| Error::SerializationError { source: err.into() })
            })
            .await;

        assert!(
            pending_event_1.is_ok(),
            "pending event should be deserialized"
        );

        let pending_event_2 = PendingEvent::builder(stream_id)
            .with_version(1)
            .with_metadata(metadata)
            .with_domain(domain_event)
            .build(|domain_event| async move {
                to_vec(&domain_event)
                    .map_err(|err| Error::SerializationError { source: err.into() })
            })
            .await;

        assert!(
            pending_event_2.is_ok(),
            "pending event 2 should be deserialized"
        );

        let record_1 = pending_event_1.unwrap();
        let record_2 = pending_event_2.unwrap();
        let result = store
            .append_to_stream(&stream_id, 2, vec![record_1, record_2])
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();

        match err {
            Error::Conflict {
                stream_id: s,
                expected_version,
            } => {
                assert_eq!(s, stream_id.to_string());
                assert_eq!(expected_version, 2);
            }
            _ => panic!("expected Conflict error"),
        }
    }
}
