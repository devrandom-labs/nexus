//! Test utilities for nexus-store. Gated behind the `testing` feature.

use crate::batch::BatchSize;
use crate::envelope::{EnvelopeError, PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
#[cfg(feature = "import")]
use crate::import::{AtomicAppend, AtomicAppendError, PlannedAppend};
use crate::notify::{NotifyError, StreamNotifiers, WakeReg};
use crate::store::{AllPosition, RawEventStore};
use crate::wake::WakeSource;
use crate::wire::{self, FrameOffsets};
use bytes::Bytes;
use nexus::ErrorId;
use nexus::Version;

use crate::stream_id::StreamKey;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::ops::Bound;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

/// Error type for the [`InMemoryStore`] adapter.
#[derive(Debug, Error)]
#[non_exhaustive]
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

/// [`InMemoryStore`]'s `$all` resume position — its
/// [`AllPosition`](crate::AllPosition).
///
/// A monotonic-but-gappy `NonZeroU64`, the in-memory analogue of fjall's
/// `GlobalSeq`. It is the key of the store's `$all` index (`global_index`), so
/// the `$all` read tags each item with the key it was stored under rather than
/// reading a position out of the (position-free) frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InMemoryAllPos(NonZeroU64);

impl InMemoryAllPos {
    /// The first position (1).
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    /// Construct from a `u64`; `None` if `v` is 0. Mirrors [`NonZeroU64::new`].
    #[must_use]
    pub const fn new(v: u64) -> Option<Self> {
        match NonZeroU64::new(v) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }

    /// The underlying integer (always >= 1).
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// The next position, or `None` on overflow at `u64::MAX`.
    const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }
}

impl AllPosition for InMemoryAllPos {}

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
    next_global_seq: Mutex<InMemoryAllPos>,
    /// All events keyed by their `$all` position ([`InMemoryAllPos`]), the
    /// `$all` read order. Holds the same `StoredFrame`s as `streams` (Arc-shared
    /// `Bytes`, cheap clones); written under `streams`'s lock in `append` so the
    /// two never diverge. The key is the authoritative position — the frame no
    /// longer carries one.
    global_index: Arc<Mutex<BTreeMap<InMemoryAllPos, StoredFrame>>>,
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
            next_global_seq: Mutex::new(InMemoryAllPos::INITIAL),
            global_index: Arc::new(Mutex::new(BTreeMap::new())),
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

/// Keyset-paginating read state for a one-shot `read_all` (`$all` order).
struct GlobalReadState {
    global_index: Arc<Mutex<BTreeMap<InMemoryAllPos, StoredFrame>>>,
    /// Exclusive lower bound: yield positions **strictly greater** than this;
    /// `None` = from the very beginning. Advances to the last-yielded position
    /// (matching the exclusive `read_all` resume contract).
    after: Option<InMemoryAllPos>,
    batch_size: usize,
    /// Position-tagged frames — the key is the authoritative `$all` position.
    buffer: VecDeque<(InMemoryAllPos, StoredFrame)>,
    done: bool,
}

impl GlobalReadState {
    async fn refill(&mut self) {
        let batch: VecDeque<(InMemoryAllPos, StoredFrame)> = {
            let guard = self.global_index.lock().await;
            let lower = self.after.map_or(Bound::Unbounded, Bound::Excluded);
            guard
                .range((lower, Bound::Unbounded))
                .take(self.batch_size)
                .map(|(pos, frame)| (*pos, frame.clone()))
                .collect()
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

/// The boxed inner stream behind [`InMemoryAllStream`] — a position-tagged
/// `$all` item stream. Aliased so the struct field reads cleanly.
type BoxedAllItemStream = core::pin::Pin<
    Box<
        dyn futures::Stream<Item = Result<(InMemoryAllPos, PersistedEnvelope), InMemoryStoreError>>
            + Send,
    >,
>;

/// `futures::Stream` of **position-tagged** envelopes over the `$all` index.
///
/// The `$all` analogue of [`InMemoryStream`]: each `Item` carries the
/// [`InMemoryAllPos`] the event was read at, so a subscriber can checkpoint a
/// position the (position-free) envelope no longer holds.
pub struct InMemoryAllStream {
    inner: BoxedAllItemStream,
}

impl futures::Stream for InMemoryAllStream {
    type Item = Result<(InMemoryAllPos, PersistedEnvelope), InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Encode a `PendingEnvelope` into a `StoredFrame`.
///
/// Delegates to [`wire::encode_frame`], which produces a 16-byte-aligned
/// payload inside the resulting [`Bytes`]. The `$all` position is **not**
/// encoded into the frame — the store keys its `global_index` by it instead.
fn encode_pending_to_frame(
    env: &PendingEnvelope,
) -> Result<StoredFrame, AppendError<InMemoryStoreError>> {
    let frame = wire::encode_frame(
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
/// Reads `schema_version` from the wire-format header at the constant offset
/// defined by [`wire`]. The `$all` position is not in the frame — callers that
/// need it read it from the `global_index` key.
fn frame_to_envelope(frame: &StoredFrame) -> Result<PersistedEnvelope, InMemoryStoreError> {
    let Some(version) = Version::new(frame.version) else {
        return Err(InMemoryStoreError::CorruptVersion);
    };
    let value = &frame.value;
    // Bytes are read individually so no fallible try_into sits in the
    // cursor hot path. The wire::encode_frame invariant guarantees these
    // offsets are present in any non-empty StoredFrame value.
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
    type AllPosition = InMemoryAllPos;
    type AllStream = InMemoryAllStream;

    async fn append(
        &self,
        id: &StreamKey,
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
                stream_id: ErrorId::from_display(id),
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
                    stream_id: ErrorId::from_display(id),
                    expected: expected_version,
                    actual: Some(env.version()),
                })?;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(id),
                    expected: Version::new(expected_env_version),
                    actual: Some(env.version()),
                });
            }
        }

        // Assign an `$all` position to each event — monotonic across all
        // streams; gaps are permitted by the `RawEventStore` contract.
        let mut counter = self.next_global_seq.lock().await;
        let mut seq = *counter;
        let mut rows: Vec<(InMemoryAllPos, StoredFrame)> = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            rows.push((seq, encode_pending_to_frame(env)?));
            seq = seq
                .next()
                .ok_or(AppendError::Store(InMemoryStoreError::GlobalSeqOverflow))?;
        }
        *counter = seq;
        drop(counter);

        // Index by `$all` position for `$all` reads, in the same critical
        // section as the per-stream store, so a reader never sees one without
        // the other.
        {
            let mut gidx = self.global_index.lock().await;
            for (pos, frame) in &rows {
                gidx.insert(*pos, frame.clone());
            }
        }

        // Store the events per-stream.
        stream.extend(rows.into_iter().map(|(_, frame)| frame));

        let should_notify = !envelopes.is_empty();

        // Release lock before notifying to avoid contention: subscribers
        // wake up and immediately try to acquire the same Mutex.
        drop(guard);

        // Wake only the subscribers parked on this stream, keyed by the
        // stable byte representation (`as_ref`), matching `subscribe`.
        if should_notify {
            self.notifiers.wake(id.as_ref());
            self.notifiers.wake_all();
        }

        Ok(())
    }

    async fn read_stream(
        &self,
        id: &StreamKey,
        from: Version,
    ) -> Result<Self::Stream, Self::Error> {
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

    async fn read_all(
        &self,
        from: Option<Self::AllPosition>,
    ) -> Result<Self::AllStream, Self::Error> {
        let state = GlobalReadState {
            global_index: Arc::clone(&self.global_index),
            // `read_all` is exclusive: yield positions strictly after `from`.
            after: from,
            batch_size: self.batch_size.get(),
            buffer: VecDeque::new(),
            done: false,
        };

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some((pos, frame)) = s.buffer.pop_front() {
                    return match frame_to_envelope(&frame) {
                        Ok(env) => {
                            // Advance the exclusive scan bound past this position.
                            s.after = Some(pos);
                            Some((Ok((pos, env)), s))
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
                s.refill().await;
                if s.buffer.is_empty() {
                    return None;
                }
            }
        });

        Ok(InMemoryAllStream {
            inner: Box::pin(futures::StreamExt::fuse(unfolded)),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WakeSource — adapter-pluggable wake for the generic subscription loop
// ═══════════════════════════════════════════════════════════════════════════

/// Delegates wake-routing to the store's inner [`StreamNotifiers`], so the
/// generic subscription loop can be tested against `InMemoryStore`.
///
/// `append` already wakes the registry after a successful in-memory commit
/// (per-stream + `$all`), so a registration armed before a concurrent append
/// is roused once that append's events are visible.
impl WakeSource for InMemoryStore {
    type Registration = WakeReg;
    type Error = NotifyError;

    fn register(&self, stream: Option<&[u8]>) -> Result<Self::Registration, Self::Error> {
        self.notifiers.register(stream)
    }

    fn wake(&self, stream: &[u8]) {
        self.notifiers.wake(stream);
        self.notifiers.wake_all();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// StreamLister — enumerate stream ids (export support, issue #145)
// ═══════════════════════════════════════════════════════════════════════════

/// Enumerate the stream ids the store holds, for export.
///
/// Takes a single snapshot of the `streams` map under one lock (atomic — no
/// torn view of the key set) and materializes the ids into a `stream::iter`
/// cursor. `InMemoryStore` is a test store, so materializing all ids at once
/// is acceptable; a real adapter (fjall, postgres) streams them lazily.
#[cfg(feature = "export")]
impl crate::export::StreamLister for InMemoryStore {
    type StreamList = futures::stream::Iter<
        std::vec::IntoIter<Result<crate::stream_id::StreamKey, InMemoryStoreError>>,
    >;

    async fn list_streams(&self) -> Result<Self::StreamList, Self::Error> {
        let ids: Vec<Result<crate::stream_id::StreamKey, InMemoryStoreError>> = {
            let guard = self.streams.lock().await;
            guard
                .keys()
                .map(|k| Ok(crate::stream_id::StreamKey::from_slice(k.as_bytes())))
                .collect()
        };
        Ok(futures::stream::iter(ids))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AtomicAppend — cross-stream atomic commit (import support, issue #145)
// ═══════════════════════════════════════════════════════════════════════════

/// Commit several per-stream runs in one atomic critical section.
///
/// Holds the `streams` lock for the whole operation — validate every write's
/// head first (no mutation), then encode + apply all — so a half-write is
/// unrepresentable and no concurrent `append` can interleave. Mirrors
/// `append`'s lock order (`streams` → `next_global_seq` → `global_index`).
#[cfg(feature = "import")]
impl AtomicAppend for InMemoryStore {
    async fn atomic_append_many(
        &self,
        writes: &[PlannedAppend],
    ) -> Result<(), AtomicAppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;

        // Phase 1 — validate every head and run shape against the RUNNING
        // per-target head; NO mutation. Tracking prior same-batch writes to a
        // target makes a non-injective route (two writes → one stream) conflict
        // here instead of concatenating into a corrupt, non-monotonic stream
        // (honours the AtomicAppend distinct-targets contract).
        let mut projected: HashMap<String, u64> = HashMap::new();
        for (index, w) in writes.iter().enumerate() {
            let key = w.target.to_string();
            let actual_raw = match projected.get(&key) {
                Some(&head) => head,
                // usize ≤ u64 on all supported (32/64-bit) targets; the map_err
                // is an unreachable belt-and-braces guard, not a Rule-3 |_|
                // discard of a meaningful error.
                None => u64::try_from(guard.get(&key).map_or(0, Vec::len))
                    .map_err(|_| AtomicAppendError::Store(InMemoryStoreError::VersionOverflow))?,
            };
            let expected_raw = w.expected_version.map_or(0, Version::as_u64);
            if actual_raw != expected_raw {
                return Err(AtomicAppendError::Conflict {
                    index,
                    actual: Version::new(actual_raw),
                });
            }
            // Defensive (CLAUDE: each crate validates at its own boundary): the
            // run must be strictly sequential from expected+1. A running counter
            // avoids any index→u64 conversion (Rule 2).
            let mut want = expected_raw.checked_add(1);
            for env in &w.events {
                let Some(want_version) = want else {
                    return Err(AtomicAppendError::Store(
                        InMemoryStoreError::VersionOverflow,
                    ));
                };
                if env.version().as_u64() != want_version {
                    return Err(AtomicAppendError::Conflict {
                        index,
                        actual: Version::new(actual_raw),
                    });
                }
                want = want_version.checked_add(1);
            }
            // Advance this target's projected head by the run just validated, so
            // a later same-target write in this batch conflicts above.
            let new_head = w
                .events
                .last()
                .map_or(actual_raw, |last| last.version().as_u64());
            projected.insert(key, new_head);
        }

        // Phase 2 — assign `$all` positions and stage frames (still no store mutation).
        let mut counter = self.next_global_seq.lock().await;
        let mut seq = *counter;
        let mut staged_streams: Vec<(String, Vec<StoredFrame>)> = Vec::with_capacity(writes.len());
        let mut staged_global: Vec<(InMemoryAllPos, StoredFrame)> = Vec::new();
        for w in writes {
            let mut frames = Vec::with_capacity(w.events.len());
            for env in &w.events {
                let frame = encode_pending_to_frame(env).map_err(|e| match e {
                    AppendError::Store(s) => AtomicAppendError::Store(s),
                    // encode_pending_to_frame only ever returns AppendError::Store
                    // (wire-format failure); it never does a head check and so can
                    // never produce Conflict. Mapped defensively so the match is
                    // exhaustive rather than relying on a private implementation
                    // detail of encode_pending_to_frame.
                    AppendError::Conflict { .. } => {
                        AtomicAppendError::Store(InMemoryStoreError::VersionOverflow)
                    }
                })?;
                staged_global.push((seq, frame.clone()));
                frames.push(frame);
                seq = seq.next().ok_or(AtomicAppendError::Store(
                    InMemoryStoreError::GlobalSeqOverflow,
                ))?;
            }
            staged_streams.push((w.target.to_string(), frames));
        }
        *counter = seq;
        drop(counter);

        // Phase 3 — commit: global index first, then per-stream (same critical
        // section, streams lock still held throughout).
        {
            let mut gidx = self.global_index.lock().await;
            for (pos, frame) in &staged_global {
                gidx.insert(*pos, frame.clone());
            }
        }
        for (key, frames) in staged_streams {
            guard.entry(key).or_default().extend(frames);
        }
        drop(guard);

        // Wake subscribers parked on each touched stream + the $all notifier.
        for w in writes {
            if !w.events.is_empty() {
                self.notifiers.wake(w.target.as_ref());
            }
        }
        self.notifiers.wake_all();
        Ok(())
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

    fn env(v: u64) -> PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E")
            .payload(vec![v as u8])
            .unwrap()
            .build()
    }

    async fn seed(store: &InMemoryStore, id: &StreamKey, count: u64) {
        for v in 1..=count {
            let expected = Version::new(v - 1);
            store.append(id, expected, &[env(v)]).await.unwrap();
        }
    }

    #[tokio::test]
    async fn read_yields_all_events_across_refills() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = StreamKey::from_slice(b"s");
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
        let id = StreamKey::from_slice(b"s");
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
        let id = StreamKey::from_slice(b"missing");
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_from_midpoint_resumes_correctly() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(3).unwrap());
        let id = StreamKey::from_slice(b"s");
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
        let id = StreamKey::from_slice(b"s");
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
mod global_read_tests {
    use super::*;
    use crate::envelope::pending_envelope;
    use crate::{Store, Subscription};
    use futures::StreamExt;

    fn sk(s: &str) -> StreamKey {
        StreamKey::from_slice(s.as_bytes())
    }

    async fn append_one(
        store: &InMemoryStore,
        id: &str,
        version: u64,
        expected: Option<u64>,
        payload: &[u8],
    ) {
        let env = pending_envelope(Version::new(version).unwrap())
            .event_type("E")
            .payload(payload.to_vec())
            .unwrap()
            .build();
        store
            .append(&sk(id), expected.and_then(Version::new), &[env])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn read_all_yields_global_order_across_streams() {
        let store = InMemoryStore::new();
        // Interleave appends across two streams: a@1, b@1, a@2.
        append_one(&store, "a", 1, None, b"a1").await;
        append_one(&store, "b", 1, None, b"b1").await;
        append_one(&store, "a", 2, Some(1), b"a2").await;

        let mut all = store.read_all(None).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.next().await {
            let (pos, env) = item.unwrap();
            seen.push((pos.as_u64(), env.payload().to_vec()));
        }
        assert_eq!(
            seen,
            vec![
                (1, b"a1".to_vec()),
                (2, b"b1".to_vec()),
                (3, b"a2".to_vec()),
            ],
            "read_all must yield every event across streams in position order"
        );
    }

    #[tokio::test]
    async fn read_all_from_is_exclusive_and_resumes() {
        let store = InMemoryStore::new();
        append_one(&store, "a", 1, None, b"a1").await;
        append_one(&store, "a", 2, Some(1), b"a2").await;
        append_one(&store, "a", 3, Some(2), b"a3").await;

        // Exclusive: strictly after position 1 → [2, 3].
        let mut all = store
            .read_all(Some(InMemoryAllPos::new(1).unwrap()))
            .await
            .unwrap();
        let mut seqs = Vec::new();
        while let Some(item) = all.next().await {
            seqs.push(item.unwrap().0.as_u64());
        }
        assert_eq!(
            seqs,
            vec![2, 3],
            "from is exclusive; position 1 and below excluded"
        );
    }

    #[tokio::test]
    async fn read_all_from_max_yields_empty() {
        // Defensive boundary (parity with fjall): nothing is strictly after
        // InMemoryAllPos::MAX, so the exclusive resume yields an empty stream —
        // no panic, no re-read of the ceiling event.
        let store = InMemoryStore::new();
        append_one(&store, "a", 1, None, b"x").await;
        let mut all = store
            .read_all(Some(InMemoryAllPos::new(u64::MAX).unwrap()))
            .await
            .unwrap();
        assert!(
            all.next().await.is_none(),
            "read_all(Some(MAX)) must yield nothing",
        );
    }

    #[tokio::test]
    async fn subscribe_all_catches_up_then_sees_live_event() {
        let store = Store::new(InMemoryStore::new());
        append_one(store.raw(), "a", 1, None, b"a1").await;
        append_one(store.raw(), "b", 1, None, b"b1").await;

        let sub = Subscription::new(&store).subscribe_all(None).unwrap();
        futures::pin_mut!(sub);
        assert_eq!(sub.next().await.unwrap().unwrap().0.as_u64(), 1);
        assert_eq!(sub.next().await.unwrap().unwrap().0.as_u64(), 2);

        let store2 = store.clone();
        tokio::spawn(async move {
            append_one(store2.raw(), "a", 2, Some(1), b"a2").await;
        });
        let (live_pos, live_env) = sub.next().await.unwrap().unwrap();
        assert_eq!(live_pos.as_u64(), 3);
        assert_eq!(live_env.payload(), b"a2");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn subscribe_all_sees_concurrent_appends_across_streams() {
        let store = Store::new(InMemoryStore::new());
        let sub = Subscription::new(&store).subscribe_all(None).unwrap();
        futures::pin_mut!(sub);

        let s1 = store.clone();
        let s2 = store.clone();
        let w1 = tokio::spawn(async move {
            for v in 1..=10 {
                append_one(s1.raw(), "x", v, (v > 1).then(|| v - 1), b"x").await;
            }
        });
        let w2 = tokio::spawn(async move {
            for v in 1..=10 {
                append_one(s2.raw(), "y", v, (v > 1).then(|| v - 1), b"y").await;
            }
        });
        w1.await.unwrap();
        w2.await.unwrap();

        let mut prev = 0u64;
        for _ in 0..20 {
            let g = sub.next().await.unwrap().unwrap().0.as_u64();
            assert!(
                g > prev,
                "$all position must be strictly increasing: {g} after {prev}"
            );
            prev = g;
        }
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
    use crate::Store;
    use crate::Subscription;
    use crate::batch::BatchSize;
    use crate::envelope::pending_envelope;
    use futures::StreamExt;

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
        let store = Store::new(InMemoryStore::with_batch_size(BatchSize::new(4).unwrap()));
        let id = StreamKey::from_slice(b"s");
        for v in 1..=40 {
            store
                .raw()
                .append(&id, Version::new(v - 1), &[env(v)])
                .await
                .unwrap();
        }

        let sub = Subscription::new(&store).subscribe(&id, None).unwrap();
        futures::pin_mut!(sub);

        for expected in 1..=40u64 {
            let got = sub.next().await.unwrap().unwrap();
            assert_eq!(got.version().as_u64(), expected);
        }

        let store2 = store.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
            store2
                .raw()
                .append(&id2, Version::new(40), &[env(41)])
                .await
                .unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.version().as_u64(), 41);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::expect_used, reason = "test code")]
mod wake_source_tests {
    use super::*;
    use crate::envelope::pending_envelope;
    use crate::wake::{WakeRegistration, WakeSource};
    use std::time::Duration;
    use tokio::time::timeout;

    /// An `append` to a stream must wake a per-stream registration armed before
    /// the append — proving `InMemoryStore`'s `WakeSource` impl routes through
    /// the same `StreamNotifiers` that `append` wakes after a commit.
    #[tokio::test]
    async fn inmemory_wakes_registration_on_append() {
        let store = InMemoryStore::new();
        let id = StreamKey::from_slice(b"s");
        let reg = WakeSource::register(&store, Some(id.as_ref())).unwrap();
        let wait = reg.arm();
        let env = pending_envelope(Version::INITIAL)
            .event_type("E")
            .payload(b"x".to_vec())
            .unwrap()
            .build();
        store.append(&id, None, &[env]).await.unwrap();
        timeout(Duration::from_secs(5), wait)
            .await
            .expect("append must wake the registration");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod all_pos_tests {
    use super::InMemoryAllPos;
    use proptest::prelude::*;

    /// `u64` strategy anchoring the project-mandated boundaries (0, 1, MAX-1,
    /// MAX) alongside a uniform interior. The `- 1` are const-folded, so they
    /// do not trip `arithmetic_side_effects`.
    fn u64_with_boundaries() -> impl Strategy<Value = u64> {
        prop_oneof![
            1 => Just(0u64),
            1 => Just(1u64),
            1 => Just(u64::MAX - 1),
            1 => Just(u64::MAX),
            10 => any::<u64>(),
        ]
    }

    #[test]
    fn new_rejects_zero() {
        // 0 is unrepresentable: an InMemoryAllPos is always >= 1 (NonZeroU64).
        assert_eq!(InMemoryAllPos::new(0), None);
    }

    #[test]
    fn initial_is_one() {
        assert_eq!(InMemoryAllPos::INITIAL.as_u64(), 1);
        assert_eq!(InMemoryAllPos::new(1), Some(InMemoryAllPos::INITIAL));
    }

    #[test]
    fn next_overflows_at_max() {
        // No successor at the ceiling — mirrors fjall's GlobalSeq so the two
        // adapters share the exclusive-resume ceiling behaviour.
        let max = InMemoryAllPos::new(u64::MAX).unwrap();
        assert_eq!(max.next(), None);
    }

    #[test]
    fn next_one_below_max_reaches_max() {
        let near = InMemoryAllPos::new(u64::MAX - 1).unwrap();
        assert_eq!(near.next(), InMemoryAllPos::new(u64::MAX));
    }

    proptest! {
        #[test]
        fn new_round_trips_nonzero(v in u64_with_boundaries()) {
            if let Some(p) = InMemoryAllPos::new(v) {
                prop_assert_eq!(p.as_u64(), v);
            } else {
                prop_assert_eq!(v, 0);
            }
        }

        #[test]
        fn ord_matches_underlying_u64(a in u64_with_boundaries(), b in u64_with_boundaries()) {
            prop_assume!(a != 0 && b != 0);
            let pa = InMemoryAllPos::new(a).unwrap();
            let pb = InMemoryAllPos::new(b).unwrap();
            prop_assert_eq!(pa.cmp(&pb), a.cmp(&b));
        }

        #[test]
        fn next_is_successor_or_none_at_ceiling(v in u64_with_boundaries()) {
            prop_assume!(v != 0);
            let p = InMemoryAllPos::new(v).unwrap();
            if let Some(n) = p.next() {
                prop_assert_eq!(n.as_u64(), v.checked_add(1).unwrap());
            } else {
                prop_assert_eq!(v, u64::MAX);
            }
        }
    }
}
