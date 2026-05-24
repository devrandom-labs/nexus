use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::pin::pin;
use std::task::Poll;

use nexus::Version;

use crate::codec::{BorrowingDecode, Decode};
use crate::envelope::PersistedEnvelope;
use crate::error::DecodeStreamError;
use crate::upcasting::{EventMorsel, Upcaster};

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
// Progress — accumulator for a checkpointed scan over an event stream
// ═══════════════════════════════════════════════════════════════════════════

/// Accumulator for a checkpointed scan over an event stream.
///
/// Carries the in-memory state plus two cursors: the last position
/// durably saved (`saved`), and the last position seen by the scan
/// (`seen`). The relationship `seen > saved` means the scan has
/// observed events that have not yet been persisted — work the
/// caller may want to flush before yielding the accumulator.
///
/// Generic over both the state type `S` and the position type `P`,
/// so the same shape works for per-stream scans (`P = Version`) and
/// multi-stream subscriptions (`P = GlobalSeq`).
///
/// # Invariants
///
/// - After [`fresh`](Progress::fresh): `saved == seen == None`.
/// - After [`resume`](Progress::resume): `saved == seen == Some(_)`.
/// - The scan body is responsible for advancing `seen` monotonically
///   as it observes events; this type does not enforce that beyond
///   the [`dirty`](Progress::dirty) / [`unsaved`](Progress::unsaved)
///   predicates relying on `Option<P>`'s derived ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress<S, P> {
    /// In-memory accumulator state.
    pub state: S,
    /// Position last durably saved.
    pub saved: Option<P>,
    /// Position last observed by the scan.
    pub seen: Option<P>,
}

impl<S, P> Progress<S, P> {
    /// Cold start — no prior cursor, no state has been observed or saved.
    ///
    /// Both `saved` and `seen` are `None`.
    pub const fn fresh(state: S) -> Self {
        Self {
            state,
            saved: None,
            seen: None,
        }
    }
}

impl<S, P: Copy> Progress<S, P> {
    /// Resume from a previously saved cursor and state.
    ///
    /// Both `saved` and `seen` are set to `saved_at` so the resumed
    /// scan starts in a non-dirty state.
    pub const fn resume(saved_at: P, state: S) -> Self {
        Self {
            state,
            saved: Some(saved_at),
            seen: Some(saved_at),
        }
    }
}

impl<S, P: Copy + Ord> Progress<S, P> {
    /// `true` when the scan has observed positions past the last save.
    ///
    /// Relies on the derived `PartialOrd` for `Option<P>`, where
    /// `None < Some(_)`. So `Some(seen) > None` (saved nothing yet,
    /// observed something) and `Some(seen) > Some(saved)` (observed
    /// beyond the saved point) both return `true`.
    pub fn dirty(&self) -> bool {
        self.seen > self.saved
    }

    /// The position to save at when [`dirty`](Self::dirty), else `None`.
    ///
    /// Returns the value the caller needs to persist alongside `state`
    /// — no separate `unwrap` of `seen` required at the call site.
    pub fn unsaved(&self) -> Option<P> {
        self.dirty().then_some(self.seen).flatten()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Step — per-iteration outcome of a checkpointed scan
// ═══════════════════════════════════════════════════════════════════════════

/// Per-iteration outcome of a checkpointed scan.
///
/// Returned by the scan body to tell the caller what to do next:
/// either persist the carried [`Progress`] before continuing, or
/// carry it forward without IO. The enum (rather than a `bool`)
/// makes the contract a `match` the compiler enforces — leave a
/// variant unhandled and the build fails.
pub enum Step<S, P> {
    /// The scan body's policy fired — caller should durably save the
    /// carried `Progress` (typically at `progress.seen`) before the
    /// next iteration.
    Save(Progress<S, P>),
    /// The scan body's policy did not fire — caller carries the
    /// `Progress` forward without performing any IO.
    Skip(Progress<S, P>),
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
enum Selected<T, E> {
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
async fn select_next_or_shutdown<NextFut, Sh, T, E>(
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

// ═══════════════════════════════════════════════════════════════════════════
// DecoderBuilder — typestate chain binding codec + upcaster to a stream
// ═══════════════════════════════════════════════════════════════════════════

/// Initial typestate marker: builder needs a codec before it can `.build()`.
pub struct NeedsCodec;

/// Typestate marker: an owning [`Decode`] reference has been bound.
pub struct WithCodec<'a, C>(&'a C);

/// Typestate marker: a [`BorrowingDecode`] reference has been bound.
pub struct WithBorrowingCodec<'a, C: ?Sized>(&'a C);

/// Typestate marker: no upcaster set; `.build()` supplies the no-op `()`.
pub struct NoUpcaster;

/// Typestate marker: an [`Upcaster`] reference has been bound.
pub struct WithUpcaster<'a, U>(&'a U);

/// Typestate builder that pairs an [`EventStream`] with a codec (required)
/// and an upcaster (optional, defaults to the no-op `()`).
///
/// Obtain via [`EventStreamExt::decoder`]. Call `.codec(&c)` or
/// `.borrowing_codec(&c)` to select the codec kind, optionally `.upcaster(&u)`
/// to bind an upcaster, then `.build()` to produce a [`DecodedStream`] or
/// [`BorrowedDecodedStream`].
///
/// The codec and upcaster are borrowed for the wrapper's lifetime — they
/// must outlive the eventual fold.
pub struct DecoderBuilder<S, CodecState, UpcasterState> {
    stream: S,
    codec: CodecState,
    upcaster: UpcasterState,
}

impl<S> DecoderBuilder<S, NeedsCodec, NoUpcaster> {
    pub(crate) const fn new(stream: S) -> Self {
        Self {
            stream,
            codec: NeedsCodec,
            upcaster: NoUpcaster,
        }
    }

    /// Bind an owning [`Decode`]. Transitions to the [`DecodedStream`] arm
    /// (owned events decoded per envelope).
    pub fn codec<C>(self, codec: &C) -> DecoderBuilder<S, WithCodec<'_, C>, NoUpcaster> {
        DecoderBuilder {
            stream: self.stream,
            codec: WithCodec(codec),
            upcaster: NoUpcaster,
        }
    }

    /// Bind a zero-copy [`BorrowingDecode`]. Transitions to the
    /// [`BorrowedDecodedStream`] arm (events borrowed from cursor payload).
    pub fn borrowing_codec<C: ?Sized>(
        self,
        codec: &C,
    ) -> DecoderBuilder<S, WithBorrowingCodec<'_, C>, NoUpcaster> {
        DecoderBuilder {
            stream: self.stream,
            codec: WithBorrowingCodec(codec),
            upcaster: NoUpcaster,
        }
    }
}

impl<'a, S, C> DecoderBuilder<S, WithCodec<'a, C>, NoUpcaster> {
    /// Bind an [`Upcaster`] to migrate event payloads before decode.
    pub fn upcaster<U>(
        self,
        upcaster: &'a U,
    ) -> DecoderBuilder<S, WithCodec<'a, C>, WithUpcaster<'a, U>> {
        DecoderBuilder {
            stream: self.stream,
            codec: self.codec,
            upcaster: WithUpcaster(upcaster),
        }
    }

    /// Finalize without an upcaster — uses the no-op `()` passthrough.
    pub fn build<M>(self) -> DecodedStream<'a, S, C, (), M> {
        static NO_UPCASTER: () = ();
        DecodedStream {
            stream: self.stream,
            codec: self.codec.0,
            upcaster: &NO_UPCASTER,
            _marker: PhantomData,
        }
    }
}

impl<'a, S, C, U> DecoderBuilder<S, WithCodec<'a, C>, WithUpcaster<'a, U>> {
    /// Finalize the chain into a [`DecodedStream`].
    pub fn build<M>(self) -> DecodedStream<'a, S, C, U, M> {
        DecodedStream {
            stream: self.stream,
            codec: self.codec.0,
            upcaster: self.upcaster.0,
            _marker: PhantomData,
        }
    }
}

impl<'a, S, C: ?Sized> DecoderBuilder<S, WithBorrowingCodec<'a, C>, NoUpcaster> {
    /// Bind an [`Upcaster`] to migrate event payloads before decode.
    pub fn upcaster<U>(
        self,
        upcaster: &'a U,
    ) -> DecoderBuilder<S, WithBorrowingCodec<'a, C>, WithUpcaster<'a, U>> {
        DecoderBuilder {
            stream: self.stream,
            codec: self.codec,
            upcaster: WithUpcaster(upcaster),
        }
    }

    /// Finalize without an upcaster — uses the no-op `()` passthrough.
    pub fn build<M>(self) -> BorrowedDecodedStream<'a, S, C, (), M> {
        static NO_UPCASTER: () = ();
        BorrowedDecodedStream {
            stream: self.stream,
            codec: self.codec.0,
            upcaster: &NO_UPCASTER,
            _marker: PhantomData,
        }
    }
}

impl<'a, S, C: ?Sized, U> DecoderBuilder<S, WithBorrowingCodec<'a, C>, WithUpcaster<'a, U>> {
    /// Finalize the chain into a [`BorrowedDecodedStream`].
    pub fn build<M>(self) -> BorrowedDecodedStream<'a, S, C, U, M> {
        BorrowedDecodedStream {
            stream: self.stream,
            codec: self.codec.0,
            upcaster: self.upcaster.0,
            _marker: PhantomData,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DecodedStream — owning-codec wrapper, yields owned `E`
// ═══════════════════════════════════════════════════════════════════════════

/// An [`EventStream`] paired with an owning [`Decode`] and [`Upcaster`].
///
/// Produced by [`DecoderBuilder::build`] after binding a [`Decode`]. The
/// [`try_fold`](Self::try_fold) method walks the underlying stream,
/// applies the upcaster to each envelope, decodes the result into an
/// owned event, and folds it through the caller-supplied closure.
///
/// Single-pass: `try_fold` consumes `self`. The codec and upcaster are
/// borrowed for the wrapper's lifetime.
pub struct DecodedStream<'a, S, C, U, M = ()> {
    stream: S,
    codec: &'a C,
    upcaster: &'a U,
    _marker: PhantomData<M>,
}

impl<S, C, U, M> DecodedStream<'_, S, C, U, M> {
    /// Fold every decoded event through `f`, short-circuiting on the first error.
    ///
    /// For each envelope yielded by the underlying stream:
    /// 1. Build an [`EventMorsel`] from the envelope's bytes.
    /// 2. Run the upcaster to migrate to the current schema.
    /// 3. Decode the (possibly transformed) payload into an owned `E`.
    /// 4. Invoke `f(acc, version, event)` to produce the next accumulator.
    ///
    /// The closure receives the envelope's stream [`Version`] alongside the
    /// decoded event — useful for replay paths that need strict-sequential
    /// validation (e.g. aggregate rehydration).
    ///
    /// # Errors
    ///
    /// Returns [`Err`] propagated from any pipeline stage:
    /// - stream cursor failure → [`DecodeStreamError::Stream`]
    /// - upcaster transform failure → [`DecodeStreamError::Upcast`]
    /// - codec decode failure → [`DecodeStreamError::Decode`]
    /// - the closure returning `Err` short-circuits the fold immediately
    pub async fn try_fold<E, B, F, Err>(mut self, init: B, mut f: F) -> Result<B, Err>
    where
        S: EventStream<M> + Send,
        C: Decode<E>,
        U: Upcaster,
        M: Send,
        B: Send,
        F: FnMut(B, Version, E) -> Result<B, Err> + Send,
        Err: From<DecodeStreamError<S::Error, C::Error, U::Error>>,
    {
        let mut acc = init;
        while let Some(env) = self
            .stream
            .next()
            .await
            .map_err(|e| Err::from(DecodeStreamError::Stream(e)))?
        {
            let version = env.version();
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );
            let transformed = self
                .upcaster
                .apply(morsel)
                .map_err(|e| Err::from(DecodeStreamError::Upcast(e)))?;
            let event = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| Err::from(DecodeStreamError::Decode(e)))?;
            acc = f(acc, version, event)?;
        }
        Ok(acc)
    }

    /// Async-fold every decoded event, interruptible by a shutdown signal.
    ///
    /// The async-closure, interruptible cousin of [`try_fold`](Self::try_fold).
    /// Adds three capabilities over the pure-sync primitive:
    ///
    /// 1. **Async closure** — the body may `.await` on per-iteration side
    ///    effects (e.g. persist projection state, save a checkpoint).
    /// 2. **Shutdown signal** — when `shutdown` resolves, the fold terminates
    ///    at the next iteration boundary and returns the accumulator with
    ///    [`Disposition::Interrupted`].
    /// 3. **Disposition return** — the caller distinguishes natural stream
    ///    completion ([`Disposition::Completed`]) from external interruption.
    ///
    /// # Pipeline (per event)
    ///
    /// 1. Build an [`EventMorsel`] from the envelope's bytes.
    /// 2. Run the upcaster to migrate to the current schema.
    /// 3. Decode the (possibly transformed) payload into an owned `E`.
    /// 4. Invoke `f(acc, version, event).await` to produce the next accumulator.
    ///
    /// # Bias
    ///
    /// On every iteration the shutdown future is polled *before* the
    /// underlying stream's `next()`. If both are ready simultaneously,
    /// shutdown wins — preventing a hot stream from starving cancellation.
    /// Mirrors fs2's `interruptWhen` semantics, where interrupt is checked
    /// via `interruptGuard` before each pull step (fs2 `Pull.scala` L950-961).
    ///
    /// # Accumulator preservation
    ///
    /// When shutdown wins, the accumulator from the *last completed
    /// iteration* is returned. An envelope whose dequeue was racing with
    /// shutdown is dropped and *not* in the result. This matches fs2's
    /// `OuterRun.interrupted` returning `F.pure(accB)` (fs2 `Pull.scala`
    /// L1283-1284) and is verified by `StreamInterruptSuite.scala` test 11.
    ///
    /// # The user closure runs to completion within an iteration
    ///
    /// Once an envelope is decoded and the closure begins executing, the
    /// closure runs to completion before shutdown is checked again. The
    /// accumulator is moved *into* the returned future; dropping the future
    /// mid-`.await` would lose it entirely. (Rust async cannot preserve a
    /// closure's local state across cancellation the way algebraic-effect
    /// handlers can.) If the closure does long-running work, it must check
    /// shutdown internally.
    ///
    /// # Cancel-safety
    ///
    /// When shutdown wins against the stream's `next()`, the in-flight
    /// `next()` future is dropped mid-poll. Implementors of [`EventStream`]
    /// **must** ensure `next()` is cancel-safe.
    ///
    /// # Errors
    ///
    /// - Closure returns `Err` → propagated immediately, no `Disposition`.
    /// - Stream/upcaster/codec returns `Err` → propagated via
    ///   `Err: From<DecodeStreamError<...>>`, no `Disposition`.
    pub async fn try_fold_async_until<E, B, F, Fut, Err, Sh>(
        mut self,
        init: B,
        mut f: F,
        shutdown: Sh,
    ) -> Result<(B, Disposition), Err>
    where
        S: EventStream<M> + Send,
        C: Decode<E>,
        U: Upcaster,
        M: Send,
        B: Send,
        F: FnMut(B, Version, E) -> Fut + Send,
        Fut: Future<Output = Result<B, Err>> + Send,
        Sh: Future<Output = ()> + Send,
        Err: From<DecodeStreamError<S::Error, C::Error, U::Error>>,
    {
        let mut pinned_shutdown = pin!(shutdown);
        let mut acc = init;
        loop {
            let next_fut = pin!(self.stream.next());
            match select_next_or_shutdown(next_fut, pinned_shutdown.as_mut()).await {
                Selected::Shutdown => return Ok((acc, Disposition::Interrupted)),
                Selected::Stream(Err(e)) => return Err(Err::from(DecodeStreamError::Stream(e))),
                Selected::Stream(Ok(None)) => return Ok((acc, Disposition::Completed)),
                Selected::Stream(Ok(Some(env))) => {
                    let version = env.version();
                    let morsel = EventMorsel::borrowed(
                        env.event_type(),
                        env.schema_version_as_version(),
                        env.payload(),
                    );
                    let transformed = self
                        .upcaster
                        .apply(morsel)
                        .map_err(|e| Err::from(DecodeStreamError::Upcast(e)))?;
                    let event = self
                        .codec
                        .decode(transformed.event_type(), transformed.payload())
                        .map_err(|e| Err::from(DecodeStreamError::Decode(e)))?;
                    acc = f(acc, version, event).await?;
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// BorrowedDecodedStream — borrowing-codec wrapper, yields &E
// ═══════════════════════════════════════════════════════════════════════════

/// An [`EventStream`] paired with a [`BorrowingDecode`] and [`Upcaster`].
///
/// Produced by [`DecoderBuilder::build`] after binding a [`BorrowingDecode`].
/// Each decoded event is yielded as `&E` borrowing from the cursor's
/// payload buffer — zero allocation per event for formats like rkyv and
/// flatbuffers.
///
/// Single-pass: `try_fold` consumes `self`.
pub struct BorrowedDecodedStream<'a, S, C: ?Sized, U, M = ()> {
    stream: S,
    codec: &'a C,
    upcaster: &'a U,
    _marker: PhantomData<M>,
}

impl<S, C: ?Sized, U, M> BorrowedDecodedStream<'_, S, C, U, M> {
    /// Fold every decoded event through `f`, short-circuiting on the first error.
    ///
    /// Same shape as [`DecodedStream::try_fold`], but the closure receives
    /// `&E` borrowed from the cursor payload rather than an owned `E`.
    /// The borrow is invalidated by the next iteration — the closure
    /// cannot retain the reference past its scope.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] propagated from any pipeline stage:
    /// - stream cursor failure → [`DecodeStreamError::Stream`]
    /// - upcaster transform failure → [`DecodeStreamError::Upcast`]
    /// - codec decode failure → [`DecodeStreamError::Decode`]
    /// - the closure returning `Err` short-circuits the fold immediately
    pub async fn try_fold<E, B, F, Err>(mut self, init: B, mut f: F) -> Result<B, Err>
    where
        S: EventStream<M> + Send,
        C: BorrowingDecode<E>,
        E: ?Sized,
        U: Upcaster,
        M: Send,
        B: Send,
        F: for<'e> FnMut(B, Version, &'e E) -> Result<B, Err> + Send,
        Err: From<DecodeStreamError<S::Error, C::Error, U::Error>>,
    {
        let mut acc = init;
        while let Some(env) = self
            .stream
            .next()
            .await
            .map_err(|e| Err::from(DecodeStreamError::Stream(e)))?
        {
            let version = env.version();
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );
            let transformed = self
                .upcaster
                .apply(morsel)
                .map_err(|e| Err::from(DecodeStreamError::Upcast(e)))?;
            let event = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| Err::from(DecodeStreamError::Decode(e)))?;
            acc = f(acc, version, event)?;
        }
        Ok(acc)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests — pure sync coverage for Progress constructors and predicates
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
#[allow(
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test code — relaxed lints"
)]
mod tests {
    use nexus::Version;

    use super::Progress;

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    // ── Progress::fresh — cold start invariants ───────────────────────

    #[test]
    fn fresh_has_no_saved_or_seen() {
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert_eq!(p.state, 0);
        assert!(p.saved.is_none());
        assert!(p.seen.is_none());
    }

    // ── Progress::resume — warm start invariants ──────────────────────

    #[test]
    fn resume_aligns_saved_and_seen() {
        let p: Progress<u64, Version> = Progress::resume(v(7), 42);
        assert_eq!(p.state, 42);
        assert_eq!(p.saved, Some(v(7)));
        assert_eq!(p.seen, Some(v(7)));
    }

    // ── Progress::dirty — three-case truth table ──────────────────────

    #[test]
    fn dirty_is_false_when_seen_is_none() {
        // Nothing observed yet → nothing to flush.
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert!(!p.dirty());
    }

    #[test]
    fn dirty_is_false_when_seen_equals_saved() {
        // Just resumed (or just saved) → aligned, nothing to flush.
        let p: Progress<u64, Version> = Progress::resume(v(3), 0);
        assert!(!p.dirty());
    }

    #[test]
    fn dirty_is_true_when_seen_is_ahead_of_saved() {
        // Saved at None, seen at Some — observed something never saved.
        let mut p: Progress<u64, Version> = Progress::fresh(0);
        p.seen = Some(v(1));
        assert!(p.dirty());

        // Saved at Some(x), seen at Some(y) with y > x — pending events
        // since last save.
        let mut p: Progress<u64, Version> = Progress::resume(v(3), 0);
        p.seen = Some(v(7));
        assert!(p.dirty());
    }

    // ── Progress::unsaved — returns the position to save at ───────────

    #[test]
    fn unsaved_returns_none_when_not_dirty() {
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert!(p.unsaved().is_none());

        let p: Progress<u64, Version> = Progress::resume(v(3), 0);
        assert!(p.unsaved().is_none());
    }

    #[test]
    fn unsaved_returns_seen_when_dirty() {
        let mut p: Progress<u64, Version> = Progress::resume(v(3), 0);
        p.seen = Some(v(7));
        assert_eq!(p.unsaved(), Some(v(7)));
    }
}
