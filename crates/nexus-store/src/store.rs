use std::sync::Arc;

use nexus::Version;

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::stream::EventStream;
use crate::stream_id::StreamKey;

// ═══════════════════════════════════════════════════════════════════════════
// Store<S> — Arc-wrapped handle to a RawEventStore backend
// ═══════════════════════════════════════════════════════════════════════════

/// Shared handle to a [`RawEventStore`] backend.
///
/// `Store` wraps the backend in an `Arc`, making it cheap to clone and
/// safe to share across tasks. It carries no codec, upcaster, or
/// aggregate binding — it is just a database handle.
///
/// Use [`repository()`](Store::repository) to obtain a
/// [`RepositoryBuilder`](crate::builder::RepositoryBuilder), then
/// configure a codec and upcaster before calling `.build()`.
///
/// # Example
///
/// ```ignore
/// // Open flows left-to-right; `.into_store()` is the de-nested `Store::new`.
/// let store = FjallStore::builder("path").open()?.into_store();
///
/// // One per-aggregate facade per aggregate; the store is the shared substrate.
/// let orders = store.repository::<Order>().codec(OrderCodec).build();
/// let users  = store.repository::<User>().codec(UserCodec).build();
/// ```
#[derive(Debug)]
pub struct Store<S> {
    inner: Arc<S>,
}

impl<S> Store<S> {
    /// Wrap a raw event store backend in a shared handle.
    pub fn new(raw: S) -> Self {
        Self {
            inner: Arc::new(raw),
        }
    }

    /// Borrow the underlying raw store.
    ///
    /// The escape hatch for users who need the substrate directly — when
    /// the [`Repository`](crate::Repository) facade's `load` / `save` isn't
    /// flexible enough (e.g. you want to filter, peek, branch, or chain
    /// custom combinators during load). Hand the borrowed `&S` to
    /// [`RawEventStore::read_stream`] / [`RawEventStore::append`] and
    /// compose your own chain via [`futures::StreamExt`] /
    /// [`futures::TryStreamExt`].
    ///
    /// Users who just want "load this aggregate" should stay on the facade.
    ///
    /// # Example
    ///
    /// Substrate-path read: convert the adapter error eagerly and drive
    /// a custom fold.
    ///
    /// ```ignore
    /// use futures::TryStreamExt;
    /// use nexus_store::{RawEventStore, Store, StreamKey};
    ///
    /// async fn count_events<S: RawEventStore>(
    ///     store: &Store<S>,
    ///     id: &StreamKey,
    ///     from: nexus::Version,
    /// ) -> Result<usize, MyError> {
    ///     let stream = store.raw().read_stream(id, from).await.map_err(MyError::Adapter)?;
    ///     stream.map_err(MyError::Adapter).try_fold(0usize, |acc, _| async move { Ok(acc + 1) }).await
    /// }
    /// ```
    ///
    /// [`RawEventStore`]: crate::RawEventStore
    /// [`RawEventStore::read_stream`]: crate::RawEventStore::read_stream
    /// [`RawEventStore::append`]: crate::RawEventStore::append
    #[must_use]
    pub fn raw(&self) -> &S {
        &self.inner
    }

    /// Borrow the inner `Arc<S>` for the subscription module's use.
    ///
    /// `pub(crate)` so the subscription module can pull the `Arc` out for
    /// [`Subscription::new`](crate::subscription::Subscription::new) without
    /// leaking `Arc` to library users. Only the `subscription` feature needs
    /// it, so it is gated to avoid a dead-code warning otherwise.
    #[cfg(feature = "subscription")]
    #[must_use]
    pub(crate) const fn arc(&self) -> &Arc<S> {
        &self.inner
    }
}

impl<S> Clone for Store<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// RawEventStore<M> — byte-level append + read_stream trait
// ═══════════════════════════════════════════════════════════════════════════

/// What database adapters implement. Bytes in, bytes out.
///
/// Knows nothing about typed events or codecs. The `EventStore` facade
/// calls this trait after encoding events into `PendingEnvelope`.
pub trait RawEventStore: Send + Sync {
    /// The error type for store operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The stream type for reading events.
    ///
    /// Owned, non-GAT, `'static` — a `futures::Stream` of
    /// `Result<PersistedEnvelope, Self::Error>`. The owned-`Bytes`
    /// envelope means cursors don't need to lend per-record; the
    /// stream's `Item` is the envelope by value.
    ///
    /// Note: the subscription path ([`Subscription::subscribe`]) requires the
    /// stream be `Unpin`. No bound is imposed here, but all shipped adapters
    /// (`ScanCursor`, `InMemoryStream`) satisfy it.
    ///
    /// [`Subscription::subscribe`]: crate::subscription::Subscription::subscribe
    type Stream: EventStream<Error = Self::Error> + 'static;

    /// The adapter-defined `$all` resume position. See [`AllPosition`].
    ///
    /// A scalar for an embedded store (fjall's `GlobalSeq`), a commit-ordered
    /// composite for a concurrent SQL store (postgres's `(txid, seq)`). It rides
    /// *alongside* `$all` events on [`AllStream`](Self::AllStream); the
    /// (position-free) [`PersistedEnvelope`] never carries it.
    type AllPosition: AllPosition;

    /// The stream type for an all-streams (`$all`) read.
    ///
    /// Owned, non-GAT, `'static` — a `futures::Stream` of
    /// `Result<(Self::AllPosition, PersistedEnvelope), Self::Error>`:
    /// **position-tagged**, ascending by [`AllPosition`], not by
    /// `(stream, version)`. The position rides on the item so resume needs no
    /// global field on the envelope. Distinct from [`Stream`](Self::Stream)
    /// because the global order is a different physical index.
    ///
    /// Note: the subscription path ([`Subscription::subscribe`]) requires the
    /// stream be `Unpin`. No bound is imposed here, but all shipped adapters
    /// (`ScanCursor`, `InMemoryStream`) satisfy it.
    ///
    /// [`Subscription::subscribe`]: crate::subscription::Subscription::subscribe
    type AllStream: futures::Stream<Item = Result<(Self::AllPosition, PersistedEnvelope), Self::Error>>
        + Send
        + 'static;

    /// Append events to a stream with optimistic concurrency.
    ///
    /// `expected_version` is the version the aggregate was at before
    /// new events were applied. The adapter checks this against the
    /// current stream version and rejects if they don't match.
    ///
    /// # Atomicity
    ///
    /// The version check and event insertion **must** be atomic. If they
    /// are separate operations (e.g. SELECT then INSERT), a concurrent
    /// writer can slip in between, corrupting the stream. Use
    /// transactions, CAS operations, or a lock to prevent this.
    ///
    /// # Implementor contract
    ///
    /// Envelopes **must** have strictly sequential versions starting from
    /// `expected_version + 1`. Implementations **must** reject batches
    /// where versions are out of order, have gaps, or contain duplicates.
    /// Accepting malformed batches corrupts the event stream.
    ///
    /// # `$all` position
    ///
    /// Each appended event is assigned an adapter-defined
    /// [`AllPosition`](Self::AllPosition) — the order an `$all` subscription
    /// resumes from. It is **not** carried on the [`PersistedEnvelope`]; it is
    /// surfaced only on the `$all` read path, tagged onto each
    /// [`AllStream`](Self::AllStream) item. The position **must** be
    /// monotonically increasing across *all* streams in commit order but is
    /// **not** required to be gapless — an adapter may skip values (e.g. after
    /// an aborted append), and readers must tolerate gaps.
    fn append(
        &self,
        id: &StreamKey,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
    ) -> impl std::future::Future<Output = Result<(), AppendError<Self::Error>>> + Send;

    /// Open a stream of events.
    ///
    /// Events are yielded one at a time as a `futures::Stream` of
    /// owned [`PersistedEnvelope`](crate::envelope::PersistedEnvelope)s.
    ///
    /// `from` is **inclusive**: the stream yields every event with
    /// `version >= from`, in ascending `Version` order, then terminates with
    /// `None`. This matches [`read_all`](Self::read_all)'s `from` semantics;
    /// the catchup seam relies on this inclusivity to resume without skipping
    /// the boundary event.
    ///
    /// # Batching
    ///
    /// An adapter **may** chunk or paginate internally (e.g. materialize a
    /// fixed number of rows at a time and keyset-resume on the stream version
    /// as the cursor drains) but is **not** required to — bounding resident
    /// memory is the adapter's concern. Whatever it does is invisible to
    /// callers: `next()` yields events in ascending `Version` order from `from`
    /// (inclusive) and returns `None` once the persisted stream is exhausted,
    /// regardless of how the events are chunked. Memory is bounded by the
    /// adapter's implementation — fjall, for instance, uses a single lazy LSM
    /// cursor rather than fixed-size batches.
    fn read_stream(
        &self,
        id: &StreamKey,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream, Self::Error>> + Send;

    /// Open a one-shot read over **all** streams, ordered by
    /// [`AllPosition`](Self::AllPosition).
    ///
    /// `from` is **exclusive**: the stream yields every event *strictly after*
    /// `from` (`None` = from the very beginning), in ascending
    /// [`AllPosition`](Self::AllPosition) order, each item **tagged** with its
    /// position, then terminates with `None`. Resume is `Ord`-based with no
    /// successor function — the live loop reopens with the last-delivered
    /// position and the adapter reads "strictly greater". The position sequence
    /// is monotonic but **not** gapless; this read tolerates gaps by scanning a
    /// range rather than stepping a successor.
    ///
    /// The exclusive `from` here is an **intentional** asymmetry with
    /// [`read_stream`](Self::read_stream)'s **inclusive** `Version` `from`
    /// (CLAUDE rule 4): a single stream has a gapless successor sequence, but a
    /// concurrent adapter's composite `$all` position has none.
    ///
    /// This is the building block under an all-streams subscription; the
    /// never-ending wait-when-caught-up behaviour is layered on top.
    ///
    /// # Batching
    ///
    /// Like [`read_stream`](Self::read_stream), an adapter **may** chunk or
    /// paginate internally (keyset-resume on the position) but is **not**
    /// required to. The externally-observable contract is unchanged: events are
    /// yielded in ascending position order strictly after `from`, the stream
    /// terminates with `None` when caught up, and resident memory is bounded by
    /// the adapter's implementation.
    fn read_all(
        &self,
        from: Option<Self::AllPosition>,
    ) -> impl std::future::Future<Output = Result<Self::AllStream, Self::Error>> + Send;

    /// Wrap this backend in a shared [`Store`] handle.
    ///
    /// The de-nested alternative to [`Store::new(self)`](Store::new): opening a
    /// store reads left-to-right —
    /// `FjallStore::builder(path).open()?.into_store()` — instead of the
    /// inside-out `Store::new(FjallStore::builder(path).open()?)`. Exactly
    /// equivalent to `Store::new`; every adapter gets it for free as a provided
    /// method, and the raw backend stays reachable via [`Store::raw`].
    #[must_use]
    fn into_store(self) -> Store<Self>
    where
        Self: Sized,
    {
        Store::new(self)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Store<S> as a delegating RawEventStore — the front door (issue #247)
// ═══════════════════════════════════════════════════════════════════════════

/// `Store<S>` is itself a [`RawEventStore`], forwarding every method to its
/// inner backend.
///
/// This makes the handle the front door: `store.append(..)` / `read_stream` /
/// `read_all` work directly, and — because [`EventExporter`] and
/// [`EventImporter`] are blanket-impl'd for every `RawEventStore` (and
/// `RawEventStore + AtomicAppend`) — `store.export_stream(..)` /
/// `store.import(..)` come for free once `Store<S>` also forwards
/// [`StreamLister`] / [`AtomicAppend`] (in the `export` / `import` modules). So
/// a `Store<S>` holder never needs `.raw()` to back up or restore, and a
/// `Store<S>` is substitutable wherever a `RawEventStore`-bounded value is
/// expected. `.raw()` remains the escape hatch for reaching the concrete `&S`.
///
/// [`EventExporter`]: crate::export::EventExporter
/// [`EventImporter`]: crate::import::EventImporter
/// [`StreamLister`]: crate::export::StreamLister
/// [`AtomicAppend`]: crate::import::AtomicAppend
impl<S: RawEventStore> RawEventStore for Store<S> {
    type Error = S::Error;
    type Stream = S::Stream;
    type AllPosition = S::AllPosition;
    type AllStream = S::AllStream;

    async fn append(
        &self,
        id: &StreamKey,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
    ) -> Result<(), AppendError<Self::Error>> {
        self.raw().append(id, expected_version, envelopes).await
    }

    async fn read_stream(
        &self,
        id: &StreamKey,
        from: Version,
    ) -> Result<Self::Stream, Self::Error> {
        self.raw().read_stream(id, from).await
    }

    async fn read_all(
        &self,
        from: Option<Self::AllPosition>,
    ) -> Result<Self::AllStream, Self::Error> {
        self.raw().read_all(from).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AllPosition — adapter-defined `$all` resume position
// ═══════════════════════════════════════════════════════════════════════════

/// Where an `$all` subscription resumes — **adapter-defined**.
///
/// A scalar for an embedded store (fjall's `GlobalSeq`), a commit-ordered
/// composite for a concurrent SQL store (postgres's `(txid, seq)`), an LSN for
/// a WAL tail. `nexus-store` owns only this trait — the *abstraction*; the
/// concrete position lives in the adapter (dependency direction: the store
/// cannot reference its adapters), and it is **never** carried on the
/// position-free [`PersistedEnvelope`].
///
/// # Carried alongside events, not derived from them
///
/// The position rides on each [`AllStream`](RawEventStore::AllStream) item as a
/// tag `(AllPosition, PersistedEnvelope)`. A consumer checkpoints the position
/// it last saw and hands it back to [`read_all`](RawEventStore::read_all) /
/// `subscribe_all` to resume — so the consumer's checkpoint type is
/// adapter-defined and must be serializable (fjall: a `u64`; postgres: a pair).
///
/// # `Ord`, no successor
///
/// The live loop resumes **strictly after** the last delivered position using
/// [`Ord`] alone. There is deliberately no `next`/successor: a composite
/// position such as `(txid, seq)` has no natural `+1` in `txid` space, and the
/// `$all` read is **exclusive** (`WHERE pos > from`), so `Ord` is all the loop
/// needs.
///
/// # Not a distributed clock
///
/// An `AllPosition` orders one store's appends; it is **not** a cross-producer
/// or causal timestamp. A distributed adapter does not widen or reinterpret it:
/// causal/HLC metadata rides in the event's `metadata` bytes, the store never
/// orders by it, and merging across producers is the consumer's job.
pub trait AllPosition: Copy + Ord + Send + Sync + core::fmt::Debug + 'static {}
