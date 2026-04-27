use std::future::Future;

use crate::envelope::PersistedEnvelope;

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
