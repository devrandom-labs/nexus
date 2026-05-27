//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::envelope::{EnvelopeError, PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::store::{GlobalSeq, RawEventStore, SubscriptionBackend};
use crate::stream::EventStream;
use bytes::Bytes;
use nexus::Id;
use nexus::Version;
use std::collections::HashMap;
use std::ops::Range;
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

    /// Envelope payload + metadata + `event_type` exceeds `u32::MAX` bytes (offset overflow).
    #[error("envelope value too large to address with Range<u32>")]
    ValueTooLarge,

    /// Persisted envelope failed integrity validation on read.
    #[error("envelope integrity error in in-memory store at version {version}")]
    EnvelopeCorrupt {
        version: u64,
        #[source]
        source: EnvelopeError,
    },
}

/// A row stored in the in-memory database.
///
/// Holds the event data in a single `Bytes` buffer (Arc-shared, cheap to clone)
/// alongside pre-computed `Range<u32>` offsets. The buffer layout is:
/// `[event_type bytes][metadata bytes (if any)][payload bytes]`.
///
/// Cloning a `StoredRow` is an Arc refcount increment plus three range copies —
/// no heap allocation.
#[derive(Clone)]
struct StoredRow {
    version: u64,
    global_seq: GlobalSeq,
    schema_version: u32,
    /// Concatenated buffer: `[event_type bytes][metadata bytes (if Some)][payload bytes]`.
    /// Cheap to clone — `Bytes` is Arc-shared.
    value: Bytes,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
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

/// Lending cursor over in-memory events.
pub struct InMemoryStream {
    events: Vec<StoredRow>,
    pos: usize,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl crate::stream::BaseEventStream for InMemoryStream {
    fn to_envelope<'a>(item: PersistedEnvelope) -> PersistedEnvelope
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for InMemoryStream {
    type Item<'a> = PersistedEnvelope;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
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
        PersistedEnvelope::try_new(
            version,
            row.global_seq,
            row.value.clone(),
            row.schema_version,
            row.event_type_range.clone(),
            row.payload_range.clone(),
            row.metadata_range.clone(),
        )
        .map(Some)
        .map_err(|source| InMemoryStoreError::EnvelopeCorrupt {
            version: row.version,
            source,
        })
    }
}

/// Encode a `PendingEnvelope` + its assigned `global_seq` into a `StoredRow`.
///
/// Lays out `[event_type][metadata?][payload]` in a single `Bytes` buffer,
/// computes `Range<u32>` offsets, and validates them via `PersistedEnvelope::try_new`.
fn encode_pending_to_row(
    env: &PendingEnvelope,
    global_seq: GlobalSeq,
) -> Result<StoredRow, AppendError<InMemoryStoreError>> {
    let event_type_bytes = env.event_type().as_bytes();
    let metadata = env.metadata();
    let payload = env.payload();

    let et_len = event_type_bytes.len();
    let meta_len = metadata.map_or(0, <[u8]>::len);
    let payload_len = payload.len();

    let total = et_len + meta_len + payload_len;
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(event_type_bytes);
    if let Some(m) = metadata {
        buf.extend_from_slice(m);
    }
    buf.extend_from_slice(payload);

    let value = Bytes::from(buf);

    let et_end =
        u32::try_from(et_len).map_err(|_| AppendError::Store(InMemoryStoreError::ValueTooLarge))?;
    let (metadata_range, payload_start) = if metadata.is_some() {
        let m_end = u32::try_from(et_len + meta_len)
            .map_err(|_| AppendError::Store(InMemoryStoreError::ValueTooLarge))?;
        (Some(et_end..m_end), m_end)
    } else {
        (None, et_end)
    };
    let payload_end =
        u32::try_from(total).map_err(|_| AppendError::Store(InMemoryStoreError::ValueTooLarge))?;

    Ok(StoredRow {
        version: env.version().as_u64(),
        global_seq,
        schema_version: env.schema_version(),
        value,
        event_type_range: 0..et_end,
        payload_range: payload_start..payload_end,
        metadata_range,
    })
}

impl RawEventStore for InMemoryStore {
    type Error = InMemoryStoreError;
    type Stream<'a> = InMemoryStream;

    async fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
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
            rows.push(encode_pending_to_row(env, seq)?);
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
                    .cloned()
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
// InMemorySubscriptionStream — Arc-owning subscription cursor
// ═══════════════════════════════════════════════════════════════════════════

/// Subscription cursor that owns an `Arc<InMemoryStore>`.
///
/// `'static` (no lifetime parameter). Spawnable across async boundaries.
/// Eagerly loads a batch of events into a local buffer, yields from that
/// buffer, then when exhausted waits on [`Notify`] and reloads. The
/// stream **never returns `None`** — it blocks until new events arrive.
///
/// The per-record [`EventStream::Item<'a>`](crate::stream::EventStream::Item)
/// GAT is `PersistedEnvelope` — `next()` yields each envelope from the
/// cursor's internal buffer.
pub struct InMemorySubscriptionStream {
    store: Arc<InMemoryStore>,
    stream_id: String,
    buffer: Vec<StoredRow>,
    pos: usize,
    last_version: Option<Version>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl InMemorySubscriptionStream {
    /// Read events from the store starting at `from_version` into the buffer.
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r.version >= from_version.as_u64())
                        .cloned()
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

impl EventStream for InMemorySubscriptionStream {
    type Item<'a>
        = PersistedEnvelope
    where
        Self: 'a;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
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

                return PersistedEnvelope::try_new(
                    version,
                    row.global_seq,
                    row.value.clone(),
                    row.schema_version,
                    row.event_type_range.clone(),
                    row.payload_range.clone(),
                    row.metadata_range.clone(),
                )
                .map(Some)
                .map_err(|source| InMemoryStoreError::EnvelopeCorrupt {
                    version: row.version,
                    source,
                });
            }

            // Buffer exhausted. Register for notification BEFORE checking
            // for new events to avoid the race where an append happens
            // between our read and our wait.
            //
            // Clone the `Arc<Notify>` so `notified` does not borrow `self`.
            // `store: Arc<InMemoryStore>` lives inside `self`, so a
            // `&mut self` call to `refill` would otherwise conflict with
            // the borrow held by `notified`.
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

impl SubscriptionBackend<()> for InMemoryStore {
    type Stream = InMemorySubscriptionStream;
    type Error = InMemoryStoreError;

    async fn subscribe(
        arc: &Arc<Self>,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<InMemorySubscriptionStream, InMemoryStoreError> {
        let stream_id = id.to_string();
        let from_version = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(InMemoryStoreError::VersionOverflow)?,
        };

        let mut sub = InMemorySubscriptionStream {
            store: Arc::clone(arc),
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
