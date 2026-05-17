use std::future::Future;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;

/// GAT lending cursor for zero-allocation event streaming.
///
/// Each call to `next()` returns a `PersistedEnvelope` that borrows
/// from the cursor's internal buffer. The previous envelope must be
/// dropped before calling `next()` again — enforced by the lifetime.
///
/// Used during aggregate rehydration where events are processed
/// one at a time (apply to state, drop, advance cursor).
///
/// # Implementor contract
///
/// Implementations **must** yield events with monotonically increasing
/// versions. That is, for consecutive calls to `next()` that return
/// `Some(Ok(envelope))`, each envelope's `version()` must be strictly
/// greater than the previous one's. Violating this invariant will cause
/// incorrect aggregate rehydration (events applied out of order).
pub trait EventStream<M = ()> {
    /// The error type for stream operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Advance the cursor and return the next event envelope.
    ///
    /// Returns `Ok(None)` when the stream is exhausted. Once this method
    /// returns `Ok(None)`, all subsequent calls must also return `Ok(None)`
    /// (fused behavior).
    ///
    /// The returned envelope borrows from `self` — drop it before
    /// calling `next()` again.
    fn next(
        &mut self,
    ) -> impl Future<Output = Result<Option<PersistedEnvelope<'_, M>>, Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// EventStreamExt — functional combinators for lending cursors
// ═══════════════════════════════════════════════════════════════════════════

/// Functional combinators for [`EventStream`].
///
/// Because `EventStream` is a GAT lending iterator (each envelope borrows
/// from the cursor), standard `Iterator` combinators don't apply. This
/// trait provides the equivalent vocabulary — fold, for-each, collect-map,
/// and count — all with short-circuiting error propagation.
///
/// Automatically available on every `EventStream` via blanket impl.
///
/// # Error handling
///
/// All methods convert stream errors via `E: From<Self::Error>`, so
/// callers don't need per-envelope `.map_err()`. The closure's error
/// type just needs a `From` impl for the stream's error type.
///
/// # Examples
///
/// ```ignore
/// use nexus_store::store::{EventStream, EventStreamExt};
///
/// // Fold events into a total payload size:
/// let total = stream.try_fold(0u64, |sum, env| {
///     Ok(sum + env.payload().len() as u64)
/// }).await?;
///
/// // Collect all payloads:
/// let payloads = stream.try_collect_map(|env| {
///     Ok(env.payload().to_vec())
/// }).await?;
/// ```
pub trait EventStreamExt<M = ()>: EventStream<M> {
    /// Fold every event into an accumulator, short-circuiting on error.
    ///
    /// This is the primitive — [`try_for_each`](Self::try_for_each),
    /// [`try_collect_map`](Self::try_collect_map), and
    /// [`try_count`](Self::try_count) are built on top of it.
    ///
    /// Stream errors are auto-converted via `E: From<Self::Error>`.
    fn try_fold<B, E, F>(&mut self, init: B, mut f: F) -> impl Future<Output = Result<B, E>> + Send
    where
        Self: Send,
        B: Send,
        F: FnMut(B, PersistedEnvelope<'_, M>) -> Result<B, E> + Send,
        E: From<Self::Error>,
    {
        async move {
            let mut acc = init;
            while let Some(env) = self.next().await.map_err(E::from)? {
                acc = f(acc, env)?;
            }
            Ok(acc)
        }
    }

    /// Process each event with a fallible closure, short-circuiting on error.
    ///
    /// Stream errors are auto-converted via `E: From<Self::Error>`.
    fn try_for_each<E, F>(&mut self, mut f: F) -> impl Future<Output = Result<(), E>> + Send
    where
        Self: Send,
        F: FnMut(PersistedEnvelope<'_, M>) -> Result<(), E> + Send,
        E: From<Self::Error>,
    {
        async move { self.try_fold((), |(), env| f(env)).await }
    }

    /// Map each event to an owned value and collect into a `Vec`.
    ///
    /// Since envelopes borrow from the cursor and can't outlive each
    /// iteration, the closure must extract an owned `T` from each
    /// envelope. This is the GAT-safe equivalent of
    /// `stream.map(f).collect()`.
    ///
    /// Stream errors are auto-converted via `E: From<Self::Error>`.
    fn try_collect_map<T, E, F>(
        &mut self,
        mut f: F,
    ) -> impl Future<Output = Result<Vec<T>, E>> + Send
    where
        Self: Send,
        T: Send,
        F: FnMut(PersistedEnvelope<'_, M>) -> Result<T, E> + Send,
        E: From<Self::Error>,
    {
        async move {
            self.try_fold(Vec::new(), |mut items, env| {
                items.push(f(env)?);
                Ok(items)
            })
            .await
        }
    }

    /// Count events in the stream, short-circuiting on the first error.
    fn try_count(&mut self) -> impl Future<Output = Result<usize, Self::Error>> + Send
    where
        Self: Send,
    {
        async move { self.try_fold(0usize, |count, _| Ok(count + 1)).await }
    }
}

/// Blanket impl — every [`EventStream`] gets [`EventStreamExt`] for free.
impl<S: EventStream<M>, M> EventStreamExt<M> for S {}

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

    /// Access the underlying raw store.
    pub(crate) fn raw(&self) -> &S {
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
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a
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
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a
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
// CheckpointStore — durable subscription positions
// ═══════════════════════════════════════════════════════════════════════════

/// Persists subscription positions for resume-after-restart.
///
/// A checkpoint records how far a subscription has processed events
/// in a stream. On restart, the subscriber loads its checkpoint and
/// passes it as `from` to [`Subscription::subscribe`].
///
/// # Contract
///
/// - `load` returns `None` for unknown subscription IDs (not an error).
/// - `save` overwrites the previous checkpoint for the given ID.
/// - Implementations must be durable — a saved checkpoint must survive
///   process restart.
pub trait CheckpointStore {
    /// The error type for checkpoint operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Load the last saved checkpoint for a subscription.
    ///
    /// Returns `None` if no checkpoint has been saved for this ID.
    fn load(
        &self,
        subscription_id: &impl Id,
    ) -> impl Future<Output = Result<Option<Version>, Self::Error>> + Send + '_;

    /// Save (overwrite) the checkpoint for a subscription.
    fn save(
        &self,
        subscription_id: &impl Id,
        version: Version,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + '_;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: CheckpointStore> CheckpointStore for &T {
    type Error = T::Error;

    fn load(
        &self,
        subscription_id: &impl Id,
    ) -> impl Future<Output = Result<Option<Version>, Self::Error>> + Send + '_ {
        (**self).load(subscription_id)
    }

    fn save(
        &self,
        subscription_id: &impl Id,
        version: Version,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + '_ {
        (**self).save(subscription_id, version)
    }
}
