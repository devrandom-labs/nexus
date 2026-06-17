use core::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::envelope::PendingEnvelope;
use crate::error::AppendError;
use crate::stream::EventStream;

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
    /// use nexus_store::{RawEventStore, Store};
    ///
    /// async fn count_events<S: RawEventStore>(
    ///     store: &Store<S>,
    ///     id: &impl nexus::Id,
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
    /// leaking `Arc` to library users.
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
    type Stream: EventStream<Error = Self::Error> + 'static;

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
        envelopes: &[PendingEnvelope],
    ) -> impl std::future::Future<Output = Result<(), AppendError<Self::Error>>> + Send;

    /// Open a stream of events.
    ///
    /// Events are yielded one at a time as a `futures::Stream` of
    /// owned [`PersistedEnvelope`](crate::envelope::PersistedEnvelope)s.
    ///
    /// # Batching
    ///
    /// An adapter materializes at most its configured `batch_size` rows at a
    /// time and refills the next batch internally as the cursor drains, by
    /// keyset resume on the stream version. This is invisible to callers:
    /// `next()` has the same contract regardless of how the events are
    /// chunked, and the stream returns `None` once the persisted stream is
    /// exhausted. The bound caps resident memory so a large stream cannot be
    /// loaded all at once.
    fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream, Self::Error>> + Send;
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
