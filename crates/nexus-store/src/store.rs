use core::fmt;
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::envelope::PendingEnvelope;
use crate::error::AppendError;
use crate::stream::{BaseEventStream, EventStream};

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
/// let store = Store::new(FjallStore::builder("path").open()?);
///
/// let orders = store.repository().codec(OrderCodec).upcaster(OrderUpcaster).build();
/// let users  = store.repository().codec(UserCodec).build();
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
    /// compose your own chain via [`EventStreamExt`](crate::EventStreamExt).
    ///
    /// Users who just want "load this aggregate" should stay on the facade.
    ///
    /// # Example
    ///
    /// Substrate-path read: convert the adapter error eagerly via
    /// [`map_err`](crate::EventStreamExt::map_err) and drive a custom fold.
    ///
    /// ```ignore
    /// use nexus_store::{EventStreamExt, RawEventStore, Store};
    ///
    /// async fn count_events<S: RawEventStore>(
    ///     store: &Store<S>,
    ///     id: &impl nexus::Id,
    ///     from: nexus::Version,
    /// ) -> Result<usize, MyError> {
    ///     store.raw()
    ///         .read_stream(id, from).await
    ///         .map_err(MyError::Adapter)?
    ///         .map_err(MyError::Adapter)
    ///         .try_count()
    ///         .await
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
pub trait RawEventStore<M = ()>: Send + Sync {
    /// The error type for store operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The lending cursor type for reading events.
    ///
    /// The [`BaseEventStream`] sub-trait bound lets facades like the
    /// decoder recover the underlying [`PersistedEnvelope`](crate::envelope::PersistedEnvelope)
    /// from `Self::Item<'_>` without an HRTB type equality (which would
    /// force `Self: 'static`).
    type Stream<'a>: BaseEventStream<M, Error = Self::Error> + 'a
    where
        Self: 'a;

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
    /// # Global sequence
    ///
    /// Each appended event is assigned a [`GlobalSeq`] — a store-local
    /// counter that increases across *all* streams in append order. The
    /// sequence **must** be monotonic but is **not** required to be
    /// gapless: an adapter may skip values (e.g. after an aborted append),
    /// and readers must tolerate gaps. The assigned value is surfaced on
    /// the read path via `PersistedEnvelope::global_seq`.
    fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), AppendError<Self::Error>>> + Send;

    /// Open a lending cursor to read events from a stream.
    ///
    /// Events are yielded one at a time via the `EventStream` trait.
    /// Each envelope borrows from the cursor — zero allocation per event.
    fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream<'_>, Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Subscription<M> — tailing stream that waits instead of terminating
// ═══════════════════════════════════════════════════════════════════════════

/// A subscription to events in a single stream.
///
/// Returns an [`EventStream`] that never exhausts — it waits for new
/// events when caught up, rather than returning `None`.
///
/// # Contract
///
/// - `from: None` → start from the beginning of the stream (version 1)
/// - `from: Some(v)` → start from the event *after* version `v`
/// - The returned stream **never returns `None`**. When all existing
///   events have been yielded, it blocks until new events are appended.
/// - Events are yielded with monotonically increasing versions, same
///   as [`EventStream`].
///
/// # Difference from `RawEventStore::read_stream`
///
/// `read_stream` returns a fused stream that terminates when caught up.
/// `subscribe` returns a stream that *waits* instead of terminating.
pub trait Subscription<M: 'static> {
    /// The subscription stream type — an [`EventStream`] that never exhausts.
    ///
    /// The stream's error type must be the same as the subscription's error type
    /// to enable uniform error handling by consumers.
    ///
    /// The stream must be `Send` so consumers (e.g. projection runners) can
    /// drive it from a multi-threaded async runtime. This is structural —
    /// every implementor must produce a `Send` stream. Pushing it to the
    /// trait level avoids HRTB bounds like `for<'a> Sub::Stream<'a>: Send`
    /// at call sites, which conflict with non-`'static` subscription borrows
    /// (the `where Self: 'a` bound on `Stream<'a>` would force the borrow's
    /// lifetime to `'static`).
    type Stream<'a>: BaseEventStream<M, Error = Self::Error> + Send + 'a
    where
        Self: 'a;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Subscribe to events in a single stream.
    fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: Subscription<M> + Sync, M: 'static> Subscription<M> for &T {
    type Stream<'a>
        = T::Stream<'a>
    where
        Self: 'a;

    type Error = T::Error;

    fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a {
        (**self).subscribe(id, from)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SharedSubscription<M> — Arc-based 'static subscription shape
// ═══════════════════════════════════════════════════════════════════════════

/// A subscription whose cursor owns an `Arc<Store>` and is therefore `'static`.
///
/// Implemented on `Arc<Store>` directly so the trait method `subscribe`
/// takes `&self` and returns a cursor that outlives the call. Each
/// subscription pays one `Arc::clone` at `subscribe()` time; per-event
/// cost is identical to the borrowed shape.
///
/// # Why this is separate from [`Subscription`]
///
/// The borrowed [`Subscription`] uses a GAT `type Stream<'a>: ... where Self: 'a`.
/// HRTB type-equality on that GAT collapses to `Self: 'static` on stable
/// Rust, so consumer sites cannot bound `for<'a> Stream<'a>::Item<'a>`
/// without a witness sub-trait ([`BaseEventStream`](crate::stream::BaseEventStream)). This trait's
/// `type Stream: EventStream<M> + Send + 'static` is a concrete, non-GAT
/// associated type — the HRTB nesting goes away. The cursor's own per-record
/// `Item<'a>` GAT on [`EventStream`](crate::stream::EventStream) is **preserved** — each `next()` still
/// lends a `PersistedEnvelope<'_>` from the cursor's internal buffer.
///
/// PR3 of the arc-subscription refactor renames this to `Subscription` and
/// deletes the borrowed shape entirely. The temporary `Shared` prefix is
/// only there to let PR1 introduce the new shape alongside the existing
/// one without breaking the workspace build.
///
/// # Contract
///
/// - `from: None` → start from the beginning of the stream (version 1)
/// - `from: Some(v)` → start from the event *after* version `v`
/// - The returned stream **never returns `None`**. When all existing
///   events have been yielded, it blocks until new events are appended.
/// - Events are yielded with monotonically increasing versions, same
///   as [`EventStream`](crate::stream::EventStream).
///
/// # Difference from [`RawEventStore::read_stream`]
///
/// `read_stream` returns a fused stream that terminates when caught up.
/// `subscribe` returns a stream that *waits* instead of terminating.
pub trait SharedSubscription<M: 'static = ()> {
    /// The subscription stream type — an [`EventStream`](crate::stream::EventStream) that never exhausts.
    ///
    /// Concrete (non-GAT) and `'static`. The cursor's own per-record
    /// `Item<'a>` GAT on [`EventStream`](crate::stream::EventStream) is preserved — borrowing happens
    /// at the per-record level, not at the subscribe-time level.
    type Stream: EventStream<M, Error = Self::Error> + Send + 'static;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Subscribe to events in a single stream.
    ///
    /// `from: None` starts from the beginning of the stream (version 1);
    /// `from: Some(v)` starts from the event *after* version `v`. The
    /// returned cursor **never returns `None`** — it waits for new events
    /// when caught up instead of terminating.
    fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// GlobalSeq — store-local global sequence number
// ═══════════════════════════════════════════════════════════════════════════

/// A store-local global sequence number.
///
/// Every event a producer appends — across *all* of its streams — receives
/// the next `GlobalSeq` at append time. It is the position an all-streams
/// subscription resumes from, the same way [`Version`] is the position
/// within a single stream.
///
/// The sequence is **monotonic but not gapless**: an aborted append may
/// burn values, so consumers must tolerate gaps and never assume
/// `next == prev + 1`.
///
/// `GlobalSeq` is a store artifact, not a domain fact. It is local to one
/// producer's store and is not portable across producers — two stores each
/// assign their own independent sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalSeq(NonZeroU64);

impl GlobalSeq {
    /// The first sequence number (1).
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    /// The next sequence number.
    ///
    /// Returns `None` if the sequence is `u64::MAX` (overflow).
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// The underlying integer value. Always >= 1.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// Construct a `GlobalSeq` from a `u64`.
    ///
    /// Returns `None` if `v` is 0. Mirrors [`NonZeroU64::new`].
    #[must_use]
    pub const fn new(v: u64) -> Option<Self> {
        match NonZeroU64::new(v) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }
}

impl fmt::Display for GlobalSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
