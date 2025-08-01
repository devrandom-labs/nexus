use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nexus::{
    domain::Id,
    error::{Error, Result},
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

#[derive(Debug, Clone)]
pub struct Store {
    #[allow(dead_code)]
    pub connection: Arc<Mutex<Connection>>,
}

impl Store {
    #[instrument(level = "debug", err)]
    pub fn new(conn: Connection) -> Result<Self> {
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

    #[inline]
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

    fn sequence_check<'a, I>(
        mut version: u64,
        events: impl IntoIterator<Item = &'a PendingEvent<I>>,
    ) -> Result<()>
    where
        I: Id,
    {
        for event in events {
            if version != event.version().get() {
                return Err(Error::SequenceMismatch {
                    stream_id: event.stream_id().to_string(),
                    expected_version: version,
                    actual_version: event.version().get(),
                });
            }
            version += 1;
        }

        Ok(())
    }
}

#[async_trait]
impl EventStore for Store {
    #[instrument(level = "debug", skip(self), err)]
    async fn append_to_stream<I>(
        &self,
        expected_version: u64,
        pending_events: Vec<PendingEvent<I>>,
    ) -> Result<()>
    where
        I: Id,
    {
        let Some(first_event) = pending_events.first() else {
            debug!("empty list of events hence, returning no-op");
            return Ok(());
        };
        let stream_id = first_event.stream_id().clone();
        let (tx, rx) = oneshot::channel::<Result<()>>();
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
                    let actual_version: u64 = tx
                        .query_row(
                            "SELECT COALESCE(MAX(version), 0) FROM event WHERE stream_id = ?1",
                            params![stream_id.as_ref()],
                            |row| row.get(0),
                        )
                        .map_err(|err| Error::Store { source: err.into() })?;

                    if actual_version != expected_version {
                        return Err(Error::Conflict {
                            stream_id: stream_id.to_string(),
                            expected_version,
                        });
                    }
                    let _ = Self::sequence_check(actual_version + 1, &pending_events)?;
                    let mut event_stmt = tx
                        .prepare_cached("INSERT INTO event (id, stream_id, version, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)")
                        .map_err(|err| Error::Store { source: err.into() })?;
                    let mut event_metadata_stmt = tx
                        .prepare_cached(
                            "INSERT INTO event_metadata (event_id, correlation_id) VALUES (?1, ?2)",
                        )
                        .map_err(|err| Error::Store { source: err.into() })?;

                    for record in &pending_events {
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
    ) -> Pin<Box<dyn Stream<Item = Result<PersistedEvent<I>>> + Send + 'a>>
    where
        Self: Sync + 'a,
        I: Id,
    {
        debug!(?stream_id, "fetching events");
        let (tx, rx) = channel::<Result<PersistedEvent<I>>>(10);
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
    use crate::Store;

    use chrono::Utc;
    use futures::TryStreamExt;
    use nexus::{
        error::{Error, Result},
        event::{BoxedEvent, EventMetadata, PendingEvent},
        infra::NexusId,
        store::EventStore,
    };
    use nexus_test_helpers::{
        TestableEvent,
        user_domain::{UserActivated, UserCreated},
    };
    use refinery::embed_migrations;
    use rusqlite::Connection;
    use serde_json::to_vec;

    embed_migrations!("migrations");

    pub struct TestContext {
        pub store: Store,
        pub stream_id: NexusId,
    }

    impl TestContext {
        pub fn new() -> Self {
            let mut conn = Connection::open_in_memory().expect("could not open connection");
            migrations::runner()
                .run(&mut conn)
                .expect("migrations could not be applied.");

            TestContext {
                store: Store::new(conn).expect("Store could not be initialized"),
                stream_id: NexusId::default(),
            }
        }
        pub async fn create_pending_event(
            &self,
            version: u64,
            event: BoxedEvent,
        ) -> Result<PendingEvent<NexusId>> {
            let metadata = EventMetadata::new("1-corr".into());
            PendingEvent::builder(self.stream_id)
                .with_version(version)?
                .with_metadata(metadata)
                .with_domain(event)
                .build(|domain_event| async move {
                    if let Some(e) = domain_event.downcast_ref::<UserActivated>() {
                        to_vec(e).map_err(|err| Error::SerializationError { source: err.into() })
                    } else if let Some(e) = domain_event.downcast_ref::<UserCreated>() {
                        to_vec(e).map_err(|err| Error::SerializationError { source: err.into() })
                    } else {
                        Err(Error::InvalidArgument {
                            name: "serialization".to_string(),
                            reason: "cannot downcast".to_string(),
                            context: "pending_event_build".to_string(),
                        })
                    }
                })
                .await
        }
    }

    #[tokio::test]
    async fn should_be_able_to_write_and_read_stream_events() {
        let ctx = TestContext::new();
        let domain_event = UserCreated {
            user_id: NexusId::default(),
            name: "Joel DSouza".to_string(),
            email: "joel@devrandom.co".to_string(),
        };
        let pending_event = ctx.create_pending_event(1, Box::new(domain_event)).await;
        assert!(
            pending_event.is_ok(),
            "pending event should be deserialized"
        );
        let expected_event = pending_event.unwrap();
        ctx.store
            .append_to_stream(0, vec![expected_event.clone()])
            .await
            .unwrap();
        let events = ctx
            .store
            .read_stream(ctx.stream_id)
            .try_collect::<Vec<_>>()
            .await
            .expect("Read stream should succeed");
        assert_eq!(events.len(), 1, "there should be 1 event to read");
        let actual_event = &events[0];
        assert_eq!(TestableEvent(expected_event), *actual_event);
        assert!(actual_event.persisted_at > (Utc::now() - chrono::Duration::seconds(5)));
    }

    #[tokio::test]
    async fn should_not_append_event_if_version_is_same() {
        let ctx = TestContext::new();
        let domain_event = UserCreated {
            user_id: NexusId::default(),
            name: "Joel DSouza".to_string(),
            email: "joel@devrandom.co".to_string(),
        };
        let pending_event_1 = ctx
            .create_pending_event(1, Box::new(domain_event.clone()))
            .await;
        assert!(pending_event_1.is_ok(), "{}", pending_event_1.unwrap_err());
        let pending_event_2 = ctx.create_pending_event(1, Box::new(domain_event)).await;
        assert!(pending_event_2.is_ok(), "{}", pending_event_1.unwrap_err());
        let record_1 = pending_event_1.unwrap();
        let record_2 = pending_event_2.unwrap();
        let result = ctx
            .store
            .append_to_stream(0, vec![record_1, record_2])
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();

        match err {
            Error::SequenceMismatch {
                stream_id,
                expected_version,
                actual_version,
            } => {
                assert_eq!(stream_id, ctx.stream_id.to_string());
                assert_eq!(expected_version, 2);
                assert_eq!(actual_version, 1);
            }
            _ => panic!("expected sequence error, but got: {err}"),
        }
    }

    #[tokio::test]
    async fn should_be_able_to_stream_events_of_stream_id() {
        let ctx = TestContext::new();
        let id = NexusId::default();
        let user_created_event = UserCreated {
            user_id: id,
            name: "Joel DSouza".to_string(),
            email: "joel@devrandom.co".to_string(),
        };
        let user_activated_event = UserActivated {};
        let pending_event_1 = ctx.create_pending_event(1, user_created_event.into()).await;
        assert!(pending_event_1.is_ok(),);
        let pending_event_2 = ctx
            .create_pending_event(2, user_activated_event.into())
            .await;
        assert!(pending_event_2.is_ok(), "{}", pending_event_2.unwrap_err());
        let record_1 = pending_event_1.unwrap();
        let record_2 = pending_event_2.unwrap();
        let result = ctx
            .store
            .append_to_stream(0, vec![record_1.clone(), record_2.clone()])
            .await;

        assert!(result.is_ok());

        let events = ctx
            .store
            .read_stream(ctx.stream_id)
            .try_collect::<Vec<_>>()
            .await
            .expect("Read stream should succeed");

        assert_eq!(events.len(), 2, "should have two events");
        let read_event_1 = &events[0];
        assert_eq!(TestableEvent(record_1), *read_event_1);
        assert!(read_event_1.persisted_at > (Utc::now() - chrono::Duration::seconds(5)));
        let read_event_2 = &events[1];
        assert_eq!(TestableEvent(record_2), *read_event_2);
        assert!(read_event_2.persisted_at > (Utc::now() - chrono::Duration::seconds(5)));
    }

    // TODO: add events and then add more events with new expected version
    // TODO: somehow test conflict error (should I rename sequence error to conflict?)

    #[tokio::test]
    async fn append_to_stream_with_empty_events_should_succeed() {
        let ctx = TestContext::new();
        let result = ctx.store.append_to_stream::<NexusId>(0, vec![]).await;
        assert!(result.is_ok());
    }
}
