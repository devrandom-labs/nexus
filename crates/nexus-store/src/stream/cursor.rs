use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;

use crate::envelope::PersistedEnvelope;

use super::decoder::{DecoderBuilder, NeedsCodec, NoUpcaster};
use super::progress::{Progress, Step};

// ═══════════════════════════════════════════════════════════════════════════
// Disposition — why an interruptible fold returned
// ═══════════════════════════════════════════════════════════════════════════

/// Why a [`try_fold_async_until`](EventStreamExt::try_fold_async_until) — or
/// the same method on [`DecodedStream`]/[`BorrowedDecodedStream`] — exited.
///
/// Returned alongside the accumulator so the caller can distinguish a stream
/// that ended naturally from one cut short by an external shutdown signal.
///
/// The accumulator is always returned in the state of the *last completed
/// iteration* — events processed before the exit are visible; an in-flight
/// `next()` that was dropped because shutdown won is **not**. This mirrors
/// fs2's `interruptWhen` + `compile.fold` contract (see fs2 `Pull.scala`
/// `OuterRun.interrupted` which returns `F.pure(accB)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The stream's `next()` returned `Ok(None)`; the fold consumed every event.
    Completed,
    /// The shutdown future resolved before the stream finished; the fold
    /// returned the accumulator at the last completed iteration.
    Interrupted,
}
// ═══════════════════════════════════════════════════════════════════════════
// EventStream — GAT lending cursor over persisted envelopes
// ═══════════════════════════════════════════════════════════════════════════

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
/// use nexus_store::stream::{EventStream, EventStreamExt};
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

    /// Fold every event into an accumulator, interruptible by a shutdown signal.
    ///
    /// The async-closure, interruptible cousin of [`try_fold`](Self::try_fold).
    /// The raw-envelope variant of
    /// [`DecodedStream::try_fold_async_until`](DecodedStream::try_fold_async_until)
    /// — use this when you want to handle decode yourself in the closure
    /// (e.g. when the caller's error type doesn't have a `From` for the full
    /// `DecodeStreamError<...>` pipeline).
    ///
    /// # Closure contract
    ///
    /// The closure receives the lent envelope and returns a `Fut` that
    /// produces the next accumulator. Because the bound is
    /// `for<'a> FnMut(B, PersistedEnvelope<'a, M>) -> Fut` with `Fut`
    /// concrete, the closure must extract owned values from the envelope
    /// *before* the `async move {}` block — the returned future must not
    /// borrow from the envelope.
    ///
    /// Typical pattern:
    /// ```ignore
    /// stream.try_fold_async_until(init, move |acc, env| {
    ///     let version = env.version();
    ///     let decoded = codec.decode(env.event_type(), env.payload());
    ///     async move {
    ///         let event = decoded.map_err(MyErr::Codec)?;
    ///         do_async_work(&event).await?;
    ///         Ok(next_acc(acc, version, event))
    ///     }
    /// }, shutdown).await
    /// ```
    ///
    /// All other semantics (bias, accumulator preservation, cancel-safety
    /// of `next()`, closure runs to completion within an iteration) match
    /// [`DecodedStream::try_fold_async_until`].
    fn try_fold_async_until<B, E, F, Fut, Sh>(
        &mut self,
        init: B,
        mut f: F,
        shutdown: Sh,
    ) -> impl Future<Output = Result<(B, Disposition), E>> + Send
    where
        Self: Send,
        B: Send,
        M: Send,
        F: FnMut(B, PersistedEnvelope<'_, M>) -> Fut + Send,
        Fut: Future<Output = Result<B, E>> + Send,
        Sh: Future<Output = ()> + Send,
        E: From<Self::Error>,
    {
        async move {
            let mut pinned_shutdown = pin!(shutdown);
            let mut acc = init;
            loop {
                let next_fut = pin!(self.next());
                let env = match select_next_or_shutdown(next_fut, pinned_shutdown.as_mut()).await {
                    Selected::Shutdown => return Ok((acc, Disposition::Interrupted)),
                    Selected::Stream(Err(e)) => return Err(E::from(e)),
                    Selected::Stream(Ok(None)) => return Ok((acc, Disposition::Completed)),
                    Selected::Stream(Ok(Some(env))) => env,
                };
                acc = f(acc, env).await?;
            }
        }
    }

    /// Scan a stream with checkpointed progress, interruptible by a shutdown signal.
    ///
    /// Threads a [`Progress<S, P>`] accumulator through the lending cursor,
    /// dispatching on the per-iteration [`Step`] outcome the `step` closure
    /// returns:
    ///
    /// - [`Step::Save`] → invoke `save(position, state)` to persist, then
    ///   continue with `saved` advanced to `seen`.
    /// - [`Step::Skip`] → continue without IO, carrying the updated progress.
    ///
    /// When the stream completes naturally or `shutdown` resolves, the
    /// flush-tail runs: if [`Progress::unsaved`] returns `Some(pos)`, one
    /// final `save(pos, state)` is invoked before the method returns. This
    /// guarantees no observed-but-unsaved work is dropped on exit regardless
    /// of [`Disposition`].
    ///
    /// # Closures
    ///
    /// `step` follows the same "extract owned values before `async move`"
    /// pattern as [`try_fold_async_until`](Self::try_fold_async_until) — the
    /// returned future must not borrow from the envelope.
    ///
    /// `save` consumes the state by value and returns it back inside its
    /// future. This avoids the borrow lifetime that would otherwise require
    /// boxing (`AsyncFnMut`'s returned-future `Send` bound is unstable on
    /// the current toolchain). Moves are size-agnostic, so a large `State`
    /// is no more expensive to thread through `save` than a small one.
    ///
    /// # Bias and cancel-safety
    ///
    /// Same as [`try_fold_async_until`](Self::try_fold_async_until): shutdown
    /// is polled before the stream's `next()` each iteration; the in-flight
    /// `next()` is dropped when shutdown wins (implementors of [`EventStream`]
    /// must keep `next()` cancel-safe). Once the `step` closure begins, it
    /// runs to completion before shutdown is re-checked.
    ///
    /// # Errors
    ///
    /// - `step` returns `Err` → propagated immediately; no flush-tail runs.
    /// - `save` returns `Err` → propagated immediately; remaining work is lost.
    /// - Stream cursor errors are auto-converted via `E: From<Self::Error>`.
    fn try_scan_until<S, P, E, StepFn, StepFut, SaveFn, SaveFut, Sh>(
        &mut self,
        initial: Progress<S, P>,
        mut step: StepFn,
        mut save: SaveFn,
        shutdown: Sh,
    ) -> impl Future<Output = Result<(Progress<S, P>, Disposition), E>> + Send
    where
        Self: Send,
        M: Send,
        S: Send,
        P: Send + Copy + Ord,
        E: From<Self::Error> + Send,
        StepFn: FnMut(Progress<S, P>, PersistedEnvelope<'_, M>) -> StepFut + Send,
        StepFut: Future<Output = Result<Step<S, P>, E>> + Send,
        SaveFn: FnMut(P, S) -> SaveFut + Send,
        SaveFut: Future<Output = Result<S, E>> + Send,
        Sh: Future<Output = ()> + Send,
    {
        async move {
            let mut pinned_shutdown = pin!(shutdown);
            let mut progress = initial;
            let disposition = loop {
                let next_fut = pin!(self.next());
                let env = match select_next_or_shutdown(next_fut, pinned_shutdown.as_mut()).await {
                    Selected::Shutdown => break Disposition::Interrupted,
                    Selected::Stream(Err(e)) => return Err(E::from(e)),
                    Selected::Stream(Ok(None)) => break Disposition::Completed,
                    Selected::Stream(Ok(Some(env))) => env,
                };
                progress = match step(progress, env).await? {
                    Step::Save(Progress { state, seen, .. }) => {
                        let saved_state = match seen {
                            Some(pos) => save(pos, state).await?,
                            None => state,
                        };
                        Progress {
                            state: saved_state,
                            saved: seen,
                            seen,
                        }
                    }
                    Step::Skip(updated) => updated,
                };
            };

            // Flush tail — runs for both Completed and Interrupted exits.
            if let Some(pos) = progress.unsaved() {
                let Progress {
                    state: prev_state,
                    seen,
                    ..
                } = progress;
                let saved_state = save(pos, prev_state).await?;
                progress = Progress {
                    state: saved_state,
                    saved: seen,
                    seen,
                };
            }

            Ok((progress, disposition))
        }
    }

    /// Enter the decoder builder chain to produce a typed decoded stream.
    ///
    /// Pairs this stream with a codec and (optionally) an upcaster, yielding
    /// a [`DecodedStream`] or [`BorrowedDecodedStream`] whose `try_fold`
    /// receives fully decoded events alongside their [`Version`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Owning codec, no upcaster:
    /// let state = stream
    ///     .decoder()
    ///     .codec(&codec)
    ///     .build()
    ///     .try_fold(initial, |s, _version, event| projector.apply(s, &event))
    ///     .await?;
    ///
    /// // Zero-copy codec with upcaster:
    /// let root = stream
    ///     .decoder()
    ///     .borrowing_codec(&codec)
    ///     .upcaster(&upcaster)
    ///     .build()
    ///     .try_fold(root, |mut r, v, e| { r.replay(v, e)?; Ok(r) })
    ///     .await?;
    /// ```
    fn decoder(self) -> DecoderBuilder<Self, NeedsCodec, NoUpcaster>
    where
        Self: Sized,
    {
        DecoderBuilder::new(self)
    }
}

/// Blanket impl — every [`EventStream`] gets [`EventStreamExt`] for free.
impl<S: EventStream<M>, M> EventStreamExt<M> for S {}

// ═══════════════════════════════════════════════════════════════════════════
// Internal: biased select between the stream's next() and a shutdown future
// ═══════════════════════════════════════════════════════════════════════════

/// Outcome of one biased poll-race between the stream cursor and shutdown.
pub(super) enum Selected<T, E> {
    /// The stream's `next()` resolved first (or alone).
    Stream(Result<Option<T>, E>),
    /// The shutdown future resolved first (or alone). The stream's `next()`
    /// future is dropped in this branch; its work is forfeit.
    Shutdown,
}

/// Poll the stream cursor and the shutdown future, biased toward shutdown.
///
/// On every wake, shutdown is polled first; if `Ready`, returns
/// [`Selected::Shutdown`] without polling the stream. Otherwise polls the
/// stream's `next()` and returns [`Selected::Stream`] on `Ready`.
///
/// The shutdown reference is reborrowed for each iteration, so a single
/// pinned shutdown future can survive many fold iterations.
pub(super) async fn select_next_or_shutdown<NextFut, Sh, T, E>(
    mut next: std::pin::Pin<&mut NextFut>,
    mut shutdown: std::pin::Pin<&mut Sh>,
) -> Selected<T, E>
where
    NextFut: Future<Output = Result<Option<T>, E>>,
    Sh: Future<Output = ()>,
{
    poll_fn(move |cx| {
        if shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Selected::Shutdown);
        }
        match next.as_mut().poll(cx) {
            Poll::Ready(r) => Poll::Ready(Selected::Stream(r)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}
