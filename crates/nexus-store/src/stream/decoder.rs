// DecoderBuilder typestate + DecodedStream + BorrowedDecodedStream —
// codec/upcaster wrappers around a raw event stream.

use std::marker::PhantomData;
use std::pin::pin;

use nexus::Version;

use crate::codec::{BorrowingDecode, Decode};
use crate::error::DecodeStreamError;
use crate::upcasting::{EventMorsel, Upcaster};

use super::cursor::{Disposition, EventStream, Selected, select_next_or_shutdown};

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
