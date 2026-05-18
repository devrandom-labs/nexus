use std::future::Future;
use std::marker::PhantomData;

use nexus::Version;

use crate::codec::{BorrowingCodec, Codec};
use crate::envelope::PersistedEnvelope;
use crate::error::DecodeStreamError;
use crate::upcasting::{EventMorsel, Upcaster};

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
// DecoderBuilder — typestate chain binding codec + upcaster to a stream
// ═══════════════════════════════════════════════════════════════════════════

/// Initial typestate marker: builder needs a codec before it can `.build()`.
pub struct NeedsCodec;

/// Typestate marker: an owning [`Codec`] reference has been bound.
pub struct WithCodec<'a, C>(&'a C);

/// Typestate marker: a [`BorrowingCodec`] reference has been bound.
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

    /// Bind an owning [`Codec`]. Transitions to the [`DecodedStream`] arm
    /// (owned events decoded per envelope).
    pub fn codec<C>(self, codec: &C) -> DecoderBuilder<S, WithCodec<'_, C>, NoUpcaster> {
        DecoderBuilder {
            stream: self.stream,
            codec: WithCodec(codec),
            upcaster: NoUpcaster,
        }
    }

    /// Bind a zero-copy [`BorrowingCodec`]. Transitions to the
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

/// An [`EventStream`] paired with an owning [`Codec`] and [`Upcaster`].
///
/// Produced by [`DecoderBuilder::build`] after binding a [`Codec`]. The
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
    /// - codec decode failure → [`DecodeStreamError::Codec`]
    /// - the closure returning `Err` short-circuits the fold immediately
    pub async fn try_fold<E, B, F, Err>(mut self, init: B, mut f: F) -> Result<B, Err>
    where
        S: EventStream<M> + Send,
        C: Codec<E>,
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
                .map_err(|e| Err::from(DecodeStreamError::Codec(e)))?;
            acc = f(acc, version, event)?;
        }
        Ok(acc)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// BorrowedDecodedStream — borrowing-codec wrapper, yields &E
// ═══════════════════════════════════════════════════════════════════════════

/// An [`EventStream`] paired with a [`BorrowingCodec`] and [`Upcaster`].
///
/// Produced by [`DecoderBuilder::build`] after binding a [`BorrowingCodec`].
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
    /// - codec decode failure → [`DecodeStreamError::Codec`]
    /// - the closure returning `Err` short-circuits the fold immediately
    pub async fn try_fold<E, B, F, Err>(mut self, init: B, mut f: F) -> Result<B, Err>
    where
        S: EventStream<M> + Send,
        C: BorrowingCodec<E>,
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
                .map_err(|e| Err::from(DecodeStreamError::Codec(e)))?;
            acc = f(acc, version, event)?;
        }
        Ok(acc)
    }
}
