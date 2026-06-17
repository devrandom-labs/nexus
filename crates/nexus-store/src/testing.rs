//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::batch::BatchSize;
use crate::envelope::{EnvelopeError, PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::notify::{NotifyError, StreamNotifiers, SubscriptionGuard};
use crate::store::{GlobalSeq, RawEventStore};
use crate::subscription::{RawSubscription, sealed};
use crate::wire::{self, FrameOffsets};
use bytes::Bytes;
use nexus::Id;
use nexus::Version;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

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

    /// Stored row carries `schema_version == 0` — structurally corrupt
    /// (the write path uses `SchemaVersion`, which is `NonZeroU32`).
    #[error("corrupt schema_version on stored row at version {version}: got 0, must be > 0")]
    CorruptSchemaVersion { version: u64 },

    /// Failed to register a per-stream subscription wake handle.
    #[error("subscription wake registration failed")]
    Subscription(#[from] NotifyError),
}

/// A frame stored in the in-memory database.
///
/// Holds the wire-format bytes ([`wire::encode_frame`]) in a single
/// Arc-shared [`Bytes`] alongside pre-computed [`FrameOffsets`]. The
/// `version` is the stream-local position, kept out of the value
/// (fjall stores it in the key; `InMemoryStore` caches it here).
///
/// Cloning a `StoredFrame` is an Arc refcount increment plus a few range
/// copies — no heap allocation.
#[derive(Clone)]
struct StoredFrame {
    version: u64,
    /// Wire-format frame (see [`wire::encode_frame`]). Payload is 16-byte aligned.
    value: Bytes,
    offsets: FrameOffsets,
}

/// In-memory event store for testing. Implements [`RawEventStore`].
///
/// Backed by `tokio::sync::Mutex<HashMap<String, Vec<StoredFrame>>>`.
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
    streams: Arc<Mutex<HashMap<String, Vec<StoredFrame>>>>,
    notifiers: Arc<StreamNotifiers>,
    next_global_seq: Mutex<GlobalSeq>,
    batch_size: BatchSize,
}

impl InMemoryStore {
    /// Create a new empty in-memory store with the default batch size.
    #[must_use]
    pub fn new() -> Self {
        Self::with_batch_size(BatchSize::DEFAULT)
    }

    /// Create a new empty in-memory store with an explicit batch size.
    #[must_use]
    pub fn with_batch_size(batch_size: BatchSize) -> Self {
        Self {
            streams: Arc::new(Mutex::new(HashMap::new())),
            notifiers: StreamNotifiers::new(),
            next_global_seq: Mutex::new(GlobalSeq::INITIAL),
            batch_size,
        }
    }

    /// The configured read / refill batch size.
    #[must_use]
    pub const fn batch_size(&self) -> BatchSize {
        self.batch_size
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Keyset-paginating read state for a one-shot `read_stream`.
struct ReadState {
    streams: Arc<Mutex<HashMap<String, Vec<StoredFrame>>>>,
    stream_id: String,
    /// Start version of the *first* batch (used until the first yield).
    from: Version,
    last_version: Option<Version>,
    batch_size: usize,
    buffer: VecDeque<StoredFrame>,
    /// Set when a refill returns fewer than `batch_size` rows — no more data.
    done: bool,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl ReadState {
    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or(Ok(self.from), |v| {
            v.next().ok_or(InMemoryStoreError::VersionOverflow)
        })
    }

    async fn refill(&mut self, from: Version) {
        let batch = {
            let guard = self.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| scan_batch(rows, from.as_u64(), self.batch_size))
                .unwrap_or_default()
        };
        self.done = batch.len() < self.batch_size;
        self.buffer = batch;
    }
}

/// `futures::Stream` of envelopes over an in-memory stream.
///
/// Loads at most `batch_size` rows at a time and keyset-resumes
/// (`last_version + 1`) when the buffer drains, terminating with `None` once a
/// refill returns a short (or empty) batch.
pub struct InMemoryStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, InMemoryStoreError>> + Send>,
    >,
}

impl futures::Stream for InMemoryStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Encode a `PendingEnvelope` + its assigned `global_seq` into a `StoredFrame`.
///
/// Delegates to [`wire::encode_frame`], which produces a 16-byte-aligned
/// payload inside the resulting [`Bytes`].
fn encode_pending_to_frame(
    env: &PendingEnvelope,
    global_seq: GlobalSeq,
) -> Result<StoredFrame, AppendError<InMemoryStoreError>> {
    let frame = wire::encode_frame(
        global_seq.as_u64(),
        env.schema_version_value(),
        &env.event_type_value(),
        &env.payload_value(),
        env.metadata_value().as_ref(),
    )
    .map_err(|e| AppendError::Store(InMemoryStoreError::Wire(e)))?;

    Ok(StoredFrame {
        version: env.version().as_u64(),
        value: frame.value,
        offsets: frame.offsets,
    })
}

/// Construct a [`PersistedEnvelope`] from a [`StoredFrame`].
///
/// Reads `global_seq` and `schema_version` from the wire-format header
/// at the constant offsets defined by [`wire`].
fn frame_to_envelope(frame: &StoredFrame) -> Result<PersistedEnvelope, InMemoryStoreError> {
    let Some(version) = Version::new(frame.version) else {
        return Err(InMemoryStoreError::CorruptVersion);
    };
    let value = &frame.value;
    // Bytes are read individually so no fallible try_into sits in the
    // cursor hot path. The wire::encode_frame invariant guarantees these
    // offsets are present in any non-empty StoredFrame value.
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
            version: frame.version,
        });
    };
    let schema_version_raw = u32::from_le_bytes([
        value[wire::SCHEMA_VERSION_OFFSET],
        value[wire::SCHEMA_VERSION_OFFSET + 1],
        value[wire::SCHEMA_VERSION_OFFSET + 2],
        value[wire::SCHEMA_VERSION_OFFSET + 3],
    ]);
    let schema_version =
        crate::value::SchemaVersion::from_u32(schema_version_raw).map_err(|_| {
            InMemoryStoreError::CorruptSchemaVersion {
                version: frame.version,
            }
        })?;

    PersistedEnvelope::try_new(
        version,
        global_seq,
        frame.value.clone(),
        schema_version,
        frame.offsets.event_type.clone(),
        frame.offsets.payload.clone(),
        frame.offsets.metadata.clone(),
    )
    .map_err(|source| InMemoryStoreError::EnvelopeCorrupt {
        version: frame.version,
        source,
    })
}

/// Collect up to `batch_size` frames with `version >= from`, in version order.
///
/// `rows` is in insertion = version order, so the matching frames are a
/// contiguous suffix; `take(batch_size)` bounds the materialized slice.
fn scan_batch(rows: &[StoredFrame], from: u64, batch_size: usize) -> VecDeque<StoredFrame> {
    rows.iter()
        .filter(|r| r.version >= from)
        .take(batch_size)
        .cloned()
        .collect()
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
            rows.push(encode_pending_to_frame(env, seq)?);
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

        // Wake only the subscribers parked on this stream, keyed by the
        // stable byte representation (`as_ref`), matching `subscribe`.
        if should_notify {
            self.notifiers.wake(id.as_ref());
        }

        Ok(())
    }

    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        let state = ReadState {
            streams: Arc::clone(&self.streams),
            stream_id: id.to_string(),
            from,
            last_version: None,
            batch_size: self.batch_size.get(),
            buffer: VecDeque::new(),
            done: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some(row) = s.buffer.pop_front() {
                    #[cfg(debug_assertions)]
                    {
                        if let Some(prev) = s.prev_version {
                            debug_assert!(
                                row.version > prev,
                                "InMemoryStream monotonicity violated: version {} not > previous {}",
                                row.version,
                                prev,
                            );
                        }
                        s.prev_version = Some(row.version);
                    }
                    return match frame_to_envelope(&row) {
                        Ok(env) => {
                            s.last_version = Some(env.version());
                            Some((Ok(env), s))
                        }
                        Err(e) => {
                            s.done = true;
                            Some((Err(e), s))
                        }
                    };
                }
                if s.done {
                    return None;
                }
                let next_from = match s.next_read_version() {
                    Ok(v) => v,
                    Err(e) => {
                        s.done = true;
                        return Some((Err(e), s));
                    }
                };
                s.refill(next_from).await;
                if s.buffer.is_empty() {
                    return None;
                }
            }
        });

        Ok(InMemoryStream {
            inner: Box::pin(futures::StreamExt::fuse(unfolded)),
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
    buffer: VecDeque<StoredFrame>,
    last_version: Option<Version>,
    batch_size: usize,
    /// Keeps this stream's wake entry registered for the cursor's whole
    /// life and supplies the per-stream `Notify`. Dropped with the cursor,
    /// which reaps the entry once the last subscriber leaves.
    guard: SubscriptionGuard,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl SubState {
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| scan_batch(rows, from_version.as_u64(), self.batch_size))
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

impl sealed::Sealed for InMemoryStore {}

impl RawSubscription for InMemoryStore {
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

        // Register the per-stream wake guard before the catch-up read, so the
        // map entry is live for any concurrent append's `wake`. Keyed by the
        // stable byte representation, matching `append`'s `wake(id.as_ref())`.
        let guard = arc.notifiers.subscribe(id.as_ref())?;

        let mut state = SubState {
            store: Arc::clone(arc),
            stream_id,
            buffer: VecDeque::new(),
            last_version: from,
            batch_size: arc.batch_size().get(),
            guard,
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
                    return match frame_to_envelope(&row) {
                        Ok(env) => {
                            s.last_version = Some(env.version());
                            Some((Ok(env), s))
                        }
                        Err(e) => Some((Err(e), s)),
                    };
                }

                // Buffer empty. Register the waiter BEFORE refilling to close
                // the lost-wakeup race. `wake` uses `notify_waiters` (no stored
                // permit) and a `Notified` only joins the waiter list when
                // polled, so `enable()` must register it before the refill read
                // — otherwise a `wake` landing during `refill` is lost. Clone
                // the `Arc` to a local so the `Notified` does not borrow `s`
                // across the refill.
                let notify = Arc::clone(s.guard.notifier());
                let notified = notify.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable();
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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod batch_config_tests {
    use super::*;
    use crate::batch::{BatchSize, DEFAULT_BATCH};

    #[test]
    fn default_store_uses_default_batch() {
        let store = InMemoryStore::new();
        assert_eq!(store.batch_size().get(), DEFAULT_BATCH);
    }

    #[test]
    fn with_batch_size_overrides_default() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(8).unwrap());
        assert_eq!(store.batch_size().get(), 8);
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "test code"
)]
mod bounded_read_tests {
    use super::*;
    use crate::batch::BatchSize;
    use crate::envelope::pending_envelope;
    use futures::StreamExt;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    fn env(v: u64) -> PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E")
            .payload(vec![v as u8])
            .unwrap()
            .build()
    }

    async fn seed(store: &InMemoryStore, id: &Tid, count: u64) {
        for v in 1..=count {
            let expected = Version::new(v - 1);
            store.append(id, expected, &[env(v)]).await.unwrap();
        }
    }

    #[tokio::test]
    async fn read_yields_all_events_across_refills() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 14).await;

        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = stream.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=14).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn read_terminates_at_exact_batch_boundary() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 4).await;

        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = stream.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn read_empty_stream_yields_nothing() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("missing".into());
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_from_midpoint_resumes_correctly() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(3).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 10).await;
        let mut stream = store
            .read_stream(&id, Version::new(6).unwrap())
            .await
            .unwrap();
        let mut seen = Vec::new();
        while let Some(item) = stream.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, vec![6, 7, 8, 9, 10]);
    }

    // Error paths in the unfold closure set `s.done = true` before returning
    // `Some((Err(_), s))`, so an errored stream goes terminal. In-memory frames
    // are always valid, so the error path can't be injected cheaply here; the
    // observable terminal property is the same one the fuse guarantees, so we
    // assert that a fully drained stream keeps returning `None`.
    #[tokio::test]
    async fn read_stays_none_after_exhaustion() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 5).await;
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut count = 0u64;
        while let Some(item) = stream.next().await {
            item.unwrap();
            count += 1;
        }
        assert_eq!(count, 5);
        // Exhausted stream must keep returning None (fused), not panic or re-yield.
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "test code"
)]
mod bounded_subscription_tests {
    use super::*;
    use crate::batch::BatchSize;
    use crate::envelope::pending_envelope;
    use crate::subscription::RawSubscription;
    use futures::StreamExt;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    fn env(v: u64) -> PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E")
            .payload(vec![v as u8])
            .unwrap()
            .build()
    }

    #[tokio::test]
    async fn subscription_drains_many_batches_then_sees_new_event() {
        // batch_size 4; pre-seed 40 (10× batch_size backlog, 10 full refills), then push 1 live.
        let store = Arc::new(InMemoryStore::with_batch_size(BatchSize::new(4).unwrap()));
        let id = Tid("s".into());
        for v in 1..=40 {
            store
                .append(&id, Version::new(v - 1), &[env(v)])
                .await
                .unwrap();
        }

        let mut sub = InMemoryStore::subscribe(&store, &id, None).await.unwrap();

        for expected in 1..=40u64 {
            let got = sub.next().await.unwrap().unwrap();
            assert_eq!(got.version().as_u64(), expected);
        }

        let store2 = Arc::clone(&store);
        let id2 = id.clone();
        tokio::spawn(async move {
            store2
                .append(&id2, Version::new(40), &[env(41)])
                .await
                .unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.version().as_u64(), 41);
    }
}
