//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::envelope::{EnvelopeError, PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::store::{GlobalSeq, RawEventStore, SubscriptionBackend};
use crate::wire::{self, RowOffsets};
use bytes::Bytes;
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

    /// Stored event has `global_seq` 0 — corrupt data (`GlobalSeq` is `NonZero`).
    #[error("stored event has global_seq 0 — corrupt data at version {version}")]
    CorruptGlobalSeq { version: u64 },

    /// Version overflow: cannot advance past `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,

    /// Global sequence overflow: cannot advance past `u64::MAX`.
    #[error("global sequence overflow: cannot advance past u64::MAX")]
    GlobalSeqOverflow,

    /// Envelope failed wire-format validation when constructing the row.
    #[error("wire-format build error in in-memory store")]
    Wire(#[source] wire::WireError),

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
/// Holds the wire-format bytes ([`wire::build_row`]) in a single
/// Arc-shared [`Bytes`] alongside pre-computed [`RowOffsets`]. The
/// `version` is the stream-local position, kept out of the value
/// (fjall stores it in the key; `InMemoryStore` caches it here).
///
/// Cloning a `StoredRow` is an Arc refcount increment plus a few range
/// copies — no heap allocation.
#[derive(Clone)]
struct StoredRow {
    version: u64,
    /// Wire-format row (see [`wire::build_row`]). Payload is 16-byte aligned.
    value: Bytes,
    offsets: RowOffsets,
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

/// `futures::Stream` of envelopes over an in-memory snapshot.
///
/// Yields each row from a pre-loaded `VecDeque<StoredRow>` in sequence.
/// `poll_next` is pure sync (no IO) — the events were materialized at
/// `read_stream()` time.
pub struct InMemoryStream {
    events: std::collections::VecDeque<StoredRow>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl futures::Stream for InMemoryStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        let Some(row) = self.events.pop_front() else {
            return core::task::Poll::Ready(None);
        };
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
        core::task::Poll::Ready(Some(row_to_envelope(&row)))
    }
}

/// Encode a `PendingEnvelope` + its assigned `global_seq` into a `StoredRow`.
///
/// Delegates to [`wire::build_row`], which produces a 16-byte-aligned
/// payload inside the resulting [`Bytes`].
fn encode_pending_to_row(
    env: &PendingEnvelope,
    global_seq: GlobalSeq,
) -> Result<StoredRow, AppendError<InMemoryStoreError>> {
    let row = wire::build_row(
        global_seq.as_u64(),
        env.schema_version(),
        env.event_type(),
        env.metadata(),
        env.payload(),
    )
    .map_err(|e| AppendError::Store(InMemoryStoreError::Wire(e)))?;

    Ok(StoredRow {
        version: env.version().as_u64(),
        value: row.value,
        offsets: row.offsets,
    })
}

/// Construct a [`PersistedEnvelope`] from a [`StoredRow`].
///
/// Reads `global_seq` and `schema_version` from the wire-format header
/// at the constant offsets defined by [`wire`].
fn row_to_envelope(row: &StoredRow) -> Result<PersistedEnvelope, InMemoryStoreError> {
    let Some(version) = Version::new(row.version) else {
        return Err(InMemoryStoreError::CorruptVersion);
    };
    let value = &row.value;
    // Bytes are read individually so no fallible try_into sits in the
    // cursor hot path. The wire::build_row invariant guarantees these
    // offsets are present in any non-empty StoredRow value.
    let global_seq_raw = u64::from_le_bytes([
        value[wire::GLOBAL_SEQ_OFFSET],
        value[wire::GLOBAL_SEQ_OFFSET + 1],
        value[wire::GLOBAL_SEQ_OFFSET + 2],
        value[wire::GLOBAL_SEQ_OFFSET + 3],
        value[wire::GLOBAL_SEQ_OFFSET + 4],
        value[wire::GLOBAL_SEQ_OFFSET + 5],
        value[wire::GLOBAL_SEQ_OFFSET + 6],
        value[wire::GLOBAL_SEQ_OFFSET + 7],
    ]);
    let Some(global_seq) = GlobalSeq::new(global_seq_raw) else {
        return Err(InMemoryStoreError::CorruptGlobalSeq {
            version: row.version,
        });
    };
    let schema_version = u32::from_le_bytes([
        value[wire::SCHEMA_VERSION_OFFSET],
        value[wire::SCHEMA_VERSION_OFFSET + 1],
        value[wire::SCHEMA_VERSION_OFFSET + 2],
        value[wire::SCHEMA_VERSION_OFFSET + 3],
    ]);

    PersistedEnvelope::try_new(
        version,
        global_seq,
        row.value.clone(),
        schema_version,
        row.offsets.event_type.clone(),
        row.offsets.payload.clone(),
        row.offsets.metadata.clone(),
    )
    .map_err(|source| InMemoryStoreError::EnvelopeCorrupt {
        version: row.version,
        source,
    })
}

impl RawEventStore for InMemoryStore {
    type Error = InMemoryStoreError;
    type Stream = InMemoryStream;

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

    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        let key = id.to_string();
        let events: std::collections::VecDeque<StoredRow> = self
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
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// InMemorySubscriptionStream — Arc-owning subscription cursor
// ═══════════════════════════════════════════════════════════════════════════

/// Inner state for the subscription's async generator.
///
/// Owns an `Arc<InMemoryStore>`, so `'static`. Drives an `unfold`-based
/// `futures::Stream` whose `poll_next` cycles through:
/// 1. yield from the local buffer,
/// 2. refill from the store,
/// 3. wait on [`Notify`] when caught up.
///
/// The resulting stream **never returns `None`** — it blocks until new
/// events arrive.
struct SubState {
    store: Arc<InMemoryStore>,
    stream_id: String,
    buffer: std::collections::VecDeque<StoredRow>,
    last_version: Option<Version>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl SubState {
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
    }

    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or_else(
            || Ok(Version::INITIAL),
            |v| v.next().ok_or(InMemoryStoreError::VersionOverflow),
        )
    }
}

/// `futures::Stream` of subscription events.
///
/// Boxes an inner `unfold`-driven stream so the type can be named (the
/// inner closure type is opaque). Owns `Arc<InMemoryStore>` and is
/// `'static` — spawnable across async boundaries.
pub struct InMemorySubscriptionStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, InMemoryStoreError>> + Send>,
    >,
}

impl futures::Stream for InMemorySubscriptionStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl SubscriptionBackend for InMemoryStore {
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

        let mut state = SubState {
            store: Arc::clone(arc),
            stream_id,
            buffer: std::collections::VecDeque::new(),
            last_version: from,
            #[cfg(debug_assertions)]
            prev_version: from.map(Version::as_u64),
        };

        // Eagerly load catch-up events so the first poll yields without IO.
        state.refill(from_version).await;

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                // Yield from buffer if available.
                if let Some(row) = s.buffer.pop_front() {
                    #[cfg(debug_assertions)]
                    {
                        if let Some(prev) = s.prev_version {
                            debug_assert!(
                                row.version > prev,
                                "Subscription monotonicity violated: version {} not > previous {}",
                                row.version,
                                prev,
                            );
                        }
                        s.prev_version = Some(row.version);
                    }
                    return match row_to_envelope(&row) {
                        Ok(env) => {
                            s.last_version = Some(env.version());
                            Some((Ok(env), s))
                        }
                        Err(e) => Some((Err(e), s)),
                    };
                }

                // Buffer empty. Register notify BEFORE refilling to avoid
                // missing a concurrent append.
                let notify = Arc::clone(&s.store.notify);
                let notified = notify.notified();
                let next_from = match s.next_read_version() {
                    Ok(v) => v,
                    Err(e) => return Some((Err(e), s)),
                };
                s.refill(next_from).await;
                if s.buffer.is_empty() {
                    notified.await;
                }
            }
        });

        Ok(InMemorySubscriptionStream {
            inner: Box::pin(unfolded),
        })
    }
}
