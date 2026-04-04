//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::error::StoreError;
use crate::raw::RawEventStore;
use crate::stream::EventStream;
use nexus::ErrorId;
use nexus::StreamId;
use nexus::Version;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// A row stored in the in-memory database.
struct StoredRow {
    version: u64,
    event_type: String,
    schema_version: u32,
    payload: Vec<u8>,
}

/// In-memory event store for testing. Implements [`RawEventStore`].
///
/// Backed by `tokio::sync::Mutex<HashMap<String, Vec<StoredRow>>>`.
/// Includes optimistic concurrency and sequential version validation.
///
/// # Limitations vs real adapters
///
/// `InMemoryStore` is useful for testing business logic but cannot
/// catch the following classes of bugs:
///
/// - **Row ordering**: Events are always returned in insertion order.
///   A real database may return rows out of order without an explicit
///   `ORDER BY`, violating the monotonic version contract.
/// - **Partial writes**: Appends are atomic (Mutex). A real database
///   may crash mid-batch, leaving a stream in an inconsistent state.
/// - **Payload size limits**: No limit on payload size. Real databases
///   have row/column size constraints.
/// - **Distributed concurrency**: Single-process only. Cannot simulate
///   multiple writers on different machines racing on the same stream.
pub struct InMemoryStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemoryStore {
    /// Create a new empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned row for the stream cursor.
struct ReadRow {
    stream_id: String,
    version: u64,
    event_type: String,
    schema_version: u32,
    payload: Vec<u8>,
}

/// Lending cursor over in-memory events.
pub struct InMemoryStream {
    events: Vec<ReadRow>,
    pos: usize,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl EventStream for InMemoryStream {
    type Error = StoreError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        #[cfg(debug_assertions)]
        {
            if let Some(prev) = self.prev_version {
                debug_assert!(
                    row.version > prev,
                    "EventStream monotonicity violated: version {} is not greater than previous {}",
                    row.version,
                    prev,
                );
            }
            self.prev_version = Some(row.version);
        }
        Some(Ok(PersistedEnvelope::new(
            &row.stream_id,
            Version::from_persisted(row.version),
            &row.event_type,
            row.schema_version,
            &row.payload,
            (),
        )))
    }
}

impl RawEventStore for InMemoryStore {
    type Error = StoreError;
    type Stream<'a> = InMemoryStream;

    #[allow(
        clippy::significant_drop_tightening,
        reason = "guard must be held across concurrency check + push"
    )]
    async fn append(
        &self,
        stream_id: &StreamId,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.as_str().to_owned()).or_default();

        // Optimistic concurrency check.
        let actual_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if actual_version != expected_version.as_u64() {
            return Err(AppendError::Conflict {
                stream_id: ErrorId::from_display(&stream_id),
                expected: expected_version,
                actual: Version::from_persisted(actual_version),
            });
        }

        // Sequential version validation: each envelope must have version
        // expected_version + 1 + i. Uses checked arithmetic to prevent
        // overflow at version boundaries near u64::MAX.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_version = expected_version
                .as_u64()
                .checked_add(1)
                .and_then(|v| v.checked_add(i_u64))
                .ok_or_else(|| AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: expected_version,
                    actual: env.version(),
                })?;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: Version::from_persisted(expected_env_version),
                    actual: env.version(),
                });
            }
        }

        // Store the events.
        for env in envelopes {
            stream.push(StoredRow {
                version: env.version().as_u64(),
                event_type: env.event_type().to_owned(),
                schema_version: env.schema_version(),
                payload: env.payload().to_vec(),
            });
        }

        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &StreamId,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(stream_id.as_str())
            .map(|rows| {
                rows.iter()
                    .filter(|r| r.version >= from.as_u64())
                    .map(|r| ReadRow {
                        stream_id: stream_id.as_str().to_owned(),
                        version: r.version,
                        event_type: r.event_type.clone(),
                        schema_version: r.schema_version,
                        payload: r.payload.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(InMemoryStream {
            events,
            pos: 0,
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
}
