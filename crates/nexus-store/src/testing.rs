//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::store::{GlobalSeq, RawEventStore, SharedSubscription, Subscription};
use crate::stream::EventStream;
use nexus::Id;
use nexus::Version;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::sync::Notify;

/// Error type for the [`InMemoryStore`] adapter.
#[derive(Debug, Error)]
pub enum InMemoryStoreError {
    /// Stored event has version 0 — corrupt data.
    #[error("stored event has version 0 — corrupt data")]
    CorruptVersion,

    /// Version overflow: cannot advance past `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,

    /// Global sequence overflow: cannot advance past `u64::MAX`.
    #[error("global sequence overflow: cannot advance past u64::MAX")]
    GlobalSeqOverflow,
}

/// A row stored in the in-memory database.
struct StoredRow {
    version: u64,
    global_seq: GlobalSeq,
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
    notify: Arc<Notify>,
    next_global_seq: Mutex<GlobalSeq>,
}

impl InMemoryStore {
    /// Create a new empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            notify: Arc::new(Notify::new()),
            next_global_seq: Mutex::new(GlobalSeq::INITIAL),
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
    version: u64,
    global_seq: GlobalSeq,
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

impl crate::stream::BaseEventStream for InMemoryStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for InMemoryStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.events.len() {
            return Ok(None);
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
        let Some(version) = Version::new(row.version) else {
            return Err(InMemoryStoreError::CorruptVersion);
        };
        Ok(Some(PersistedEnvelope::new_unchecked(
            version,
            row.global_seq,
            &row.event_type,
            row.schema_version,
            &row.payload,
            (),
        )))
    }
}

impl RawEventStore for InMemoryStore {
    type Error = InMemoryStoreError;
    type Stream<'a> = InMemoryStream;

    async fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let key = id.to_string();
        let stream = guard.entry(key).or_default();

        // Optimistic concurrency check.
        // actual_version_raw is the number of events in the stream (0 = empty).
        // expected_version: None = new stream (expect 0 events), Some(v) = expect v events.
        let actual_version_raw = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        let expected_raw = expected_version.map_or(0, nexus::Version::as_u64);
        if actual_version_raw != expected_raw {
            return Err(AppendError::Conflict {
                stream_id: id.to_label(),
                expected: expected_version,
                actual: Version::new(actual_version_raw),
            });
        }

        // Sequential version validation: each envelope must have version
        // expected_raw + 1 + i. Uses checked arithmetic to prevent
        // overflow at version boundaries near u64::MAX.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_version = expected_raw
                .checked_add(1)
                .and_then(|v| v.checked_add(i_u64))
                .ok_or_else(|| AppendError::Conflict {
                    stream_id: id.to_label(),
                    expected: expected_version,
                    actual: Some(env.version()),
                })?;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: id.to_label(),
                    expected: Version::new(expected_env_version),
                    actual: Some(env.version()),
                });
            }
        }

        // Assign a store-global sequence to each event — monotonic across
        // all streams; gaps are permitted by the `RawEventStore` contract.
        let mut counter = self.next_global_seq.lock().await;
        let mut seq = *counter;
        let mut rows = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            rows.push(StoredRow {
                version: env.version().as_u64(),
                global_seq: seq,
                event_type: env.event_type().to_owned(),
                schema_version: env.schema_version(),
                payload: env.payload().to_vec(),
            });
            seq = seq
                .next()
                .ok_or(AppendError::Store(InMemoryStoreError::GlobalSeqOverflow))?;
        }
        *counter = seq;
        drop(counter);

        // Store the events.
        stream.extend(rows);

        let should_notify = !envelopes.is_empty();

        // Release lock before notifying to avoid contention: subscribers
        // wake up and immediately try to acquire the same Mutex.
        drop(guard);

        if should_notify {
            self.notify.notify_waiters();
        }

        Ok(())
    }

    async fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let key = id.to_string();
        let events = self
            .streams
            .lock()
            .await
            .get(&key)
            .map(|rows| {
                rows.iter()
                    .filter(|r| r.version >= from.as_u64())
                    .map(|r| ReadRow {
                        version: r.version,
                        global_seq: r.global_seq,
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

// ═══════════════════════════════════════════════════════════════════════════
// Subscription — live event streaming with catch-up
// ═══════════════════════════════════════════════════════════════════════════

/// Lending cursor for subscription streams.
///
/// Eagerly loads a batch of events into a local buffer (same pattern as
/// [`InMemoryStream`]), yields from that buffer, then when exhausted
/// waits on [`Notify`] and reloads. The stream **never returns `None`**.
pub struct InMemorySubscriptionStream<'a> {
    store: &'a InMemoryStore,
    stream_id: String,
    /// Eagerly-loaded batch of events from the last read.
    buffer: Vec<ReadRow>,
    pos: usize,
    /// Last version yielded -- for re-reading from the right position.
    last_version: Option<Version>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl InMemorySubscriptionStream<'_> {
    /// Read events from the store starting at `from_version` into the buffer.
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r.version >= from_version.as_u64())
                        .map(|r| ReadRow {
                            version: r.version,
                            global_seq: r.global_seq,
                            event_type: r.event_type.clone(),
                            schema_version: r.schema_version,
                            payload: r.payload.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        self.buffer = buffer;
        self.pos = 0;
    }

    /// Compute the version to read from next: `last_version` + 1, or `INITIAL`.
    ///
    /// Returns an error on overflow instead of silently wrapping back
    /// to `Version::INITIAL`.
    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or_else(
            || Ok(Version::INITIAL),
            |v| v.next().ok_or(InMemoryStoreError::VersionOverflow),
        )
    }
}

impl crate::stream::BaseEventStream for InMemorySubscriptionStream<'_> {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for InMemorySubscriptionStream<'_> {
    type Item<'a>
        = PersistedEnvelope<'a>
    where
        Self: 'a;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        loop {
            // Yield from the current buffer if available.
            if self.pos < self.buffer.len() {
                let row = &self.buffer[self.pos];
                self.pos += 1;

                #[cfg(debug_assertions)]
                {
                    if let Some(prev) = self.prev_version {
                        debug_assert!(
                            row.version > prev,
                            "Subscription monotonicity violated: version {} is not greater than previous {}",
                            row.version,
                            prev,
                        );
                    }
                    self.prev_version = Some(row.version);
                }

                let Some(version) = Version::new(row.version) else {
                    return Err(InMemoryStoreError::CorruptVersion);
                };
                self.last_version = Some(version);

                return Ok(Some(PersistedEnvelope::new_unchecked(
                    version,
                    row.global_seq,
                    &row.event_type,
                    row.schema_version,
                    &row.payload,
                    (),
                )));
            }

            // Buffer exhausted. Register for notification BEFORE checking
            // for new events to avoid the race where an append happens
            // between our read and our wait.
            let notified = self.store.notify.notified();

            let from = self.next_read_version()?;
            self.refill(from).await;

            // If refill found new events, loop back to yield them.
            if !self.buffer.is_empty() {
                continue;
            }

            // No new events — wait for notification, then retry.
            notified.await;
        }
    }
}

impl Subscription<()> for InMemoryStore {
    type Stream<'a> = InMemorySubscriptionStream<'a>;
    type Error = InMemoryStoreError;

    async fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> Result<InMemorySubscriptionStream<'a>, InMemoryStoreError> {
        let stream_id = id.to_string();
        let from_version = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(InMemoryStoreError::VersionOverflow)?,
        };

        let mut sub = InMemorySubscriptionStream {
            store: self,
            stream_id,
            buffer: Vec::new(),
            pos: 0,
            last_version: from,
            #[cfg(debug_assertions)]
            prev_version: from.map(Version::as_u64),
        };

        // Eagerly load catch-up events.
        sub.refill(from_version).await;

        Ok(sub)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SharedInMemorySubscriptionStream — Arc-based cursor (PR1 of arc-subscription refactor)
// ═══════════════════════════════════════════════════════════════════════════

/// Subscription cursor that owns an `Arc<InMemoryStore>` instead of borrowing.
///
/// `'static` (no lifetime parameter). Spawnable across async boundaries.
/// PR3 of the arc-subscription refactor renames this to
/// `InMemorySubscriptionStream` and deletes the borrowed variant.
///
/// The per-record [`EventStream::Item<'a>`](crate::stream::EventStream::Item)
/// GAT is `PersistedEnvelope<'a>` — `next()` still lends each envelope
/// from the cursor's internal buffer, exactly as the borrowed variant.
/// Only the subscribe-time borrow from store is replaced by `Arc::clone`.
pub struct SharedInMemorySubscriptionStream {
    store: Arc<InMemoryStore>,
    stream_id: String,
    buffer: Vec<ReadRow>,
    pos: usize,
    last_version: Option<Version>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl SharedInMemorySubscriptionStream {
    /// Read events from the store starting at `from_version` into the buffer.
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r.version >= from_version.as_u64())
                        .map(|r| ReadRow {
                            version: r.version,
                            global_seq: r.global_seq,
                            event_type: r.event_type.clone(),
                            schema_version: r.schema_version,
                            payload: r.payload.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        self.buffer = buffer;
        self.pos = 0;
    }

    /// Compute the version to read from next: `last_version` + 1, or `INITIAL`.
    ///
    /// Returns an error on overflow instead of silently wrapping back
    /// to `Version::INITIAL`.
    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or_else(
            || Ok(Version::INITIAL),
            |v| v.next().ok_or(InMemoryStoreError::VersionOverflow),
        )
    }
}

impl EventStream for SharedInMemorySubscriptionStream {
    type Item<'a>
        = PersistedEnvelope<'a>
    where
        Self: 'a;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        loop {
            // Yield from the current buffer if available.
            if self.pos < self.buffer.len() {
                let row = &self.buffer[self.pos];
                self.pos += 1;

                #[cfg(debug_assertions)]
                {
                    if let Some(prev) = self.prev_version {
                        debug_assert!(
                            row.version > prev,
                            "Subscription monotonicity violated: version {} is not greater than previous {}",
                            row.version,
                            prev,
                        );
                    }
                    self.prev_version = Some(row.version);
                }

                let Some(version) = Version::new(row.version) else {
                    return Err(InMemoryStoreError::CorruptVersion);
                };
                self.last_version = Some(version);

                return Ok(Some(PersistedEnvelope::new_unchecked(
                    version,
                    row.global_seq,
                    &row.event_type,
                    row.schema_version,
                    &row.payload,
                    (),
                )));
            }

            // Buffer exhausted. Register for notification BEFORE checking
            // for new events to avoid the race where an append happens
            // between our read and our wait.
            //
            // Clone the Arc<Notify> so `notified` does not borrow `self`.
            // The borrowed variant can take `self.store.notify.notified()`
            // directly because `store: &'a InMemoryStore` is external;
            // here `store: Arc<InMemoryStore>` lives inside `self`, so a
            // `&mut self` call to `refill` would conflict with the borrow.
            let notify = Arc::clone(&self.store.notify);
            let notified = notify.notified();

            let from = self.next_read_version()?;
            self.refill(from).await;

            // If refill found new events, loop back to yield them.
            if !self.buffer.is_empty() {
                continue;
            }

            // No new events — wait for notification, then retry.
            notified.await;
        }
    }
}

impl SharedSubscription<()> for Arc<InMemoryStore> {
    type Stream = SharedInMemorySubscriptionStream;
    type Error = InMemoryStoreError;

    async fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<SharedInMemorySubscriptionStream, InMemoryStoreError> {
        let stream_id = id.to_string();
        let from_version = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(InMemoryStoreError::VersionOverflow)?,
        };

        let mut sub = SharedInMemorySubscriptionStream {
            store: Self::clone(self),
            stream_id,
            buffer: Vec::new(),
            pos: 0,
            last_version: from,
            #[cfg(debug_assertions)]
            prev_version: from.map(Version::as_u64),
        };

        // Eagerly load catch-up events.
        sub.refill(from_version).await;

        Ok(sub)
    }
}
