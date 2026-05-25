use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::pin::pin;
use std::task::Poll;

use super::combinators::{Map, TryMap, TryScan};
use super::decoder::{DecoderBuilder, NeedsCodec, NoUpcaster};
#[cfg(feature = "futures-bridge")]
use super::owned::{IntoStream, OwnedEventStream};
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
/// Each call to `next()` returns an item — by default a
/// [`PersistedEnvelope`] — that borrows from the cursor's internal
/// buffer. The previous item must be dropped before calling `next()`
/// again — enforced by the lifetime on the [`Item`](EventStream::Item)
/// GAT.
///
/// The base stream impls (`InMemoryStream`, `FjallStream`, the
/// subscription cursors) set `Item<'a> = PersistedEnvelope<'a, M>`.
/// Combinators on [`EventStreamExt`] (e.g. [`map`](EventStreamExt::map),
/// [`try_map`](EventStreamExt::try_map),
/// [`try_scan`](EventStreamExt::try_scan)) project that to different
/// shapes — owned values from a closure or state-borrowed views from a
/// scan.
///
/// Used during aggregate rehydration where events are processed
/// one at a time (apply to state, drop, advance cursor).
///
/// # Implementor contract
///
/// Base impls (cursors over a real backing store) **must** yield events
/// with monotonically increasing versions. That is, for consecutive
/// calls to `next()` that return `Some(Ok(envelope))`, each envelope's
/// `version()` must be strictly greater than the previous one's.
/// Violating this invariant will cause incorrect aggregate rehydration
/// (events applied out of order).
///
/// Combinators are pure relays — they preserve order and propagate
/// monotonicity from the wrapped stream.
pub trait EventStream<M = ()> {
    /// The item type yielded by `next()`. The base cursors set this to
    /// `PersistedEnvelope<'a, M>`; combinators redefine it (e.g.
    /// `Map<S, F>::Item<'a>` is the closure output).
    ///
    /// The `where Self: 'a` clause is the lending-iterator standard — the
    /// item may borrow from `&'a mut self`, so it must not outlive
    /// `self`. Code that needs HRTB constraints like
    /// "`Item<'a>` *is* `PersistedEnvelope<'a, M>`" should phrase them as
    /// trait bounds, not type equalities (e.g.
    /// `for<'a> S::Item<'a>: IntoPersistedEnvelope<'a, M>`), because the
    /// type-equality form leaks the `Self: 'a` requirement into the HRTB
    /// and forces `Self: 'static`.
    type Item<'a>
    where
        Self: 'a;

    /// The error type for stream operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Advance the cursor and return the next item.
    ///
    /// Returns `Ok(None)` when the stream is exhausted. Once this method
    /// returns `Ok(None)`, all subsequent calls must also return `Ok(None)`
    /// (fused behavior).
    ///
    /// The returned item may borrow from `self` (the default `Item<'a>`,
    /// [`PersistedEnvelope`], does) — drop it before calling `next()`
    /// again.
    fn next(&mut self) -> impl Future<Output = Result<Option<Self::Item<'_>>, Self::Error>> + Send;
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
    /// Fold every item into an accumulator, short-circuiting on error.
    ///
    /// This is the primitive — [`try_for_each`](Self::try_for_each),
    /// [`try_collect_map`](Self::try_collect_map), and
    /// [`try_count`](Self::try_count) are built on top of it.
    ///
    /// The closure receives the stream's [`Item`](EventStream::Item) —
    /// for the base cursors that's a [`PersistedEnvelope`], for
    /// combinator-wrapped streams whatever the combinator projects to.
    ///
    /// Stream errors are auto-converted via `E: From<Self::Error>`.
    fn try_fold<B, E, F>(&mut self, init: B, mut f: F) -> impl Future<Output = Result<B, E>> + Send
    where
        Self: Send,
        B: Send,
        F: for<'a> FnMut(B, Self::Item<'a>) -> Result<B, E> + Send,
        E: From<Self::Error>,
    {
        async move {
            let mut acc = init;
            while let Some(item) = self.next().await.map_err(E::from)? {
                acc = f(acc, item)?;
            }
            Ok(acc)
        }
    }

    /// Process each item with a fallible closure, short-circuiting on error.
    ///
    /// Stream errors are auto-converted via `E: From<Self::Error>`.
    fn try_for_each<E, F>(&mut self, mut f: F) -> impl Future<Output = Result<(), E>> + Send
    where
        Self: Send,
        F: for<'a> FnMut(Self::Item<'a>) -> Result<(), E> + Send,
        E: From<Self::Error>,
    {
        async move { self.try_fold((), |(), item| f(item)).await }
    }

    /// Map each item to an owned value and collect into a `Vec`.
    ///
    /// Since stream items may borrow from the cursor and can't outlive
    /// each iteration, the closure must extract an owned `T` from each
    /// item. This is the GAT-safe equivalent of
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
        F: for<'a> FnMut(Self::Item<'a>) -> Result<T, E> + Send,
        E: From<Self::Error>,
    {
        async move {
            self.try_fold(Vec::new(), |mut items, item| {
                items.push(f(item)?);
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
    /// The closure receives the lent item and returns a `Fut` that
    /// produces the next accumulator. Because the bound is
    /// `for<'a> FnMut(B, Self::Item<'a>) -> Fut` with `Fut` concrete,
    /// the closure must extract owned values from the item *before* the
    /// `async move {}` block — the returned future must not borrow from
    /// the item.
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
        F: for<'a> FnMut(B, Self::Item<'a>) -> Fut + Send,
        Fut: Future<Output = Result<B, E>> + Send,
        Sh: Future<Output = ()> + Send,
        E: From<Self::Error>,
    {
        async move {
            let mut pinned_shutdown = pin!(shutdown);
            let mut acc = init;
            loop {
                let next_fut = pin!(self.next());
                let item = match select_next_or_shutdown(next_fut, pinned_shutdown.as_mut()).await {
                    Selected::Shutdown => return Ok((acc, Disposition::Interrupted)),
                    Selected::Stream(Err(e)) => return Err(E::from(e)),
                    Selected::Stream(Ok(None)) => return Ok((acc, Disposition::Completed)),
                    Selected::Stream(Ok(Some(item))) => item,
                };
                acc = f(acc, item).await?;
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
    /// returned future must not borrow from the item.
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
        StepFn: for<'a> FnMut(Progress<S, P>, Self::Item<'a>) -> StepFut + Send,
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
                let item = match select_next_or_shutdown(next_fut, pinned_shutdown.as_mut()).await {
                    Selected::Shutdown => break Disposition::Interrupted,
                    Selected::Stream(Err(e)) => return Err(E::from(e)),
                    Selected::Stream(Ok(None)) => break Disposition::Completed,
                    Selected::Stream(Ok(Some(item))) => item,
                };
                progress = match step(progress, item).await? {
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

    /// Project each yielded item to an owned `T` via a closure.
    ///
    /// The closure bound is `for<'a> FnMut(Self::Item<'a>) -> T` — `T`
    /// cannot borrow from the lent item (use [`try_scan`](Self::try_scan)
    /// for borrowing-output cases). The returned [`Map`] is itself an
    /// [`EventStream`] (with `Item<'a> = T`), so combinators chain:
    /// `stream.map(f).try_map(g).try_fold(init, h)`.
    fn map<F, T>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: for<'a> FnMut(Self::Item<'a>) -> T,
    {
        Map { inner: self, f }
    }

    /// Fallible variant of [`map`](Self::map).
    ///
    /// The closure returns `Result<T, E>`; underlying stream errors are
    /// auto-converted via `E: From<Self::Error>`. Closure errors and stream
    /// errors both surface from the wrapped stream's `next()`.
    fn try_map<F, T, E>(self, f: F) -> TryMap<Self, F, E>
    where
        Self: Sized,
        F: for<'a> FnMut(Self::Item<'a>) -> Result<T, E>,
        E: From<Self::Error>,
    {
        TryMap {
            inner: self,
            f,
            _err: PhantomData,
        }
    }

    /// Stateful scan — the **borrowing-output** combinator.
    ///
    /// Threads `&mut State` plus each lent item through `f`, which returns
    /// a borrowed reference into `state`. The wrapped stream's `Item<'a>`
    /// becomes `&'a T` (borrow tied to the scan's own state, not to the
    /// closure stack), so callers can decode into a buffer and yield
    /// references into it without HRTB helper-trait stacks.
    ///
    /// Use this for zero-copy decode and any pipeline where the
    /// per-iteration output must outlive the source item but is
    /// invalidated by the next iteration.
    fn try_scan<State, F, T, E>(self, state: State, f: F) -> TryScan<Self, State, F, T, E>
    where
        Self: Sized,
        T: ?Sized,
        F: for<'s, 'a> FnMut(&'s mut State, Self::Item<'a>) -> Result<&'s T, E>,
        E: From<Self::Error>,
    {
        TryScan {
            inner: self,
            state,
            f,
            _marker: PhantomData,
        }
    }

    /// Bridge an owning-output lending stream into a [`futures_core::Stream`].
    ///
    /// Available only with the `futures-bridge` feature. Requires
    /// [`OwnedEventStream<M>`] — implemented for the owning combinators
    /// ([`Map`], [`TryMap`]) but **not** for raw cursors, whose `Item<'a>`
    /// borrows from the cursor. To bridge a raw cursor, chain
    /// `.map(...)` or `.try_map(...)` first so the per-event materialization
    /// is named at the call site.
    ///
    /// The returned stream yields `Result<Self::OwnedItem, Self::Error>` and
    /// terminates after the underlying cursor returns `Ok(None)` or surfaces
    /// an error (the error is yielded once; subsequent polls return `None`).
    ///
    /// # Cost
    ///
    /// Each yielded item box-pins one async block holding the cursor — the
    /// per-event allocation that makes the futures-stream combinator
    /// surface available. If you don't need the futures ecosystem, stay
    /// in lending-stream land (`.try_fold`, `.try_for_each`,
    /// `.try_collect_map`) and pay no allocation.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use futures::StreamExt;
    /// use nexus_store::stream::{EventStream, EventStreamExt};
    ///
    /// let owned: Vec<(Version, Vec<u8>)> = my_stream
    ///     .try_map(|env| Ok::<_, MyErr>((env.version(), env.payload().to_vec())))
    ///     .into_stream()
    ///     .try_collect()
    ///     .await?;
    /// ```
    #[cfg(feature = "futures-bridge")]
    fn into_stream(self) -> IntoStream<Self, M>
    where
        Self: Sized + OwnedEventStream<M> + Send + 'static,
    {
        IntoStream::new(self)
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
