use nexus::{ErrorId, KernelError, Version};
use thiserror::Error;

/// Errors from the event store layer.
///
/// Generic over adapter (`A`), encode (`EncErr`), and decode (`DecErr`) error
/// types — zero allocation, no `Box<dyn Error>`.
///
/// `EncErr` and `DecErr` are independent so write-only and read-only codecs
/// can each set the unused side to `Infallible`. For the common case where
/// a single underlying format powers both directions, the implementor picks
/// the same `Error` associated type on both `Encode` and `Decode` impls and
/// `EncErr == DecErr` falls out without a where-clause.
///
/// Upcast errors are *not* part of this type — the no-upcaster
/// [`load`](crate::Repository::load) / [`save`](crate::Repository::save)
/// path can't produce them. When the user calls
/// [`EventStore::load_with`](crate::EventStore::load_with) (passing an
/// upcast function), the result wraps `StoreError` in
/// [`LoadWithError`] alongside the user's upcast error type.
#[derive(Debug, Error)]
pub enum StoreError<A, EncErr, DecErr> {
    /// Optimistic concurrency conflict.
    #[error(
        "concurrency conflict on stream '{stream_id}': expected version {expected:?}, actual {actual:?}"
    )]
    Conflict {
        stream_id: ErrorId,
        expected: Option<Version>,
        actual: Option<Version>,
    },

    /// Stream not found.
    #[error("stream '{stream_id}' not found")]
    StreamNotFound { stream_id: ErrorId },

    /// Database adapter failure.
    #[error("adapter error: {0}")]
    Adapter(#[source] A),

    /// Serialization failure on the write path.
    #[error("encode error: {0}")]
    Encode(#[source] EncErr),

    /// Deserialization failure on the read path.
    #[error("decode error: {0}")]
    Decode(#[source] DecErr),

    /// Kernel error during replay (e.g. version mismatch, rehydration limit).
    #[error("kernel error: {0}")]
    Kernel(#[from] KernelError),

    /// Version overflow: cannot advance past `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,

    /// Failure while synthesizing a fresh envelope for codec decode.
    ///
    /// Reachable only from upcaster-driven paths
    /// ([`EventStore::load_with`](crate::EventStore::load_with),
    /// [`ZeroCopyEventStore::load_with`](crate::ZeroCopyEventStore::load_with)):
    /// after the user's upcast transforms the event, a fresh aligned
    /// envelope is built from the transformed `event_type` + payload via
    /// [`PersistedEnvelope::for_decode`](crate::PersistedEnvelope::for_decode).
    /// The build can fail at the value-newtype boundary (oversize
    /// `event_type`/`payload`), the wire encode (`FrameLengthOverflow`),
    /// or the envelope construction (range invariants).
    #[error("envelope synthesis error: {0}")]
    EnvelopeSynthesis(#[source] crate::envelope::ForDecodeError),

    /// Envelope construction rejected user-supplied bytes
    /// (payload exceeded its size cap, or another value-newtype invariant).
    ///
    /// Raised on the save path when an encoded payload violates the
    /// invariants enforced by the [`PendingEnvelope`](crate::PendingEnvelope)
    /// builder. The schema-version-zero case is not reachable from the
    /// typed-repository save path because `SchemaVersion` is constructed
    /// from `NonZeroU32`.
    #[error("envelope error: {0}")]
    Envelope(#[from] crate::envelope::EnvelopeError),
}

impl<A, EncErr, DecErr> StoreError<A, EncErr, DecErr> {
    /// Returns `true` if this is an optimistic-concurrency [`Conflict`].
    ///
    /// [`Conflict`] is the one error the store can assert a *semantic* fact
    /// about that the consumer cannot infer otherwise: the expected version
    /// was stale, so reloading the aggregate and re-running the (pure,
    /// side-effect-free) decision will likely succeed. It is the natural
    /// predicate for a consumer-owned retry loop — e.g. a supervising actor
    /// retrying load → handle → save, or a `tower::retry::Policy` /
    /// `backon` `.when(|e| e.is_conflict())`.
    ///
    /// nexus deliberately ships no retry machinery: classifying *other*
    /// errors as retryable (transient adapter I/O, say) needs context only
    /// the consumer has, and the loop, backoff, and sleep are runtime
    /// concerns. This predicate is the entire retry-facing surface.
    ///
    /// [`Conflict`]: StoreError::Conflict
    #[must_use]
    pub const fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict { .. })
    }
}

/// Errors from the with-upcaster load path.
///
/// Returned by [`EventStore::load_with`](crate::EventStore::load_with) and
/// [`ZeroCopyEventStore::load_with`](crate::ZeroCopyEventStore::load_with).
/// Wraps the four error sources [`StoreError`] already carries plus the
/// user-supplied upcast function's error type.
///
/// `LoadWithError<A, EncErr, DecErr, UpErr>` is structurally `StoreError +
/// Upcast(UpErr)`. The `From<StoreError<A, EncErr, DecErr>>` impl lets the
/// `?` operator promote a `StoreError` into the wider variant inside a
/// `load_with` body without manual matching.
#[derive(Debug, Error)]
pub enum LoadWithError<A, EncErr, DecErr, UpErr> {
    /// All non-upcast errors — wrapped verbatim from the no-upcaster path.
    #[error(transparent)]
    Store(#[from] StoreError<A, EncErr, DecErr>),

    /// Upcast function failure — carries the user-supplied error verbatim.
    /// No wrapper is applied; encode diagnostic context (event type, schema
    /// version) in your own error type if needed.
    #[error("upcast error: {0}")]
    Upcast(#[source] UpErr),
}

impl<A, EncErr, DecErr, UpErr> From<KernelError> for LoadWithError<A, EncErr, DecErr, UpErr> {
    fn from(err: KernelError) -> Self {
        Self::Store(StoreError::Kernel(err))
    }
}

/// Structured error from [`RawEventStore::append`](crate::RawEventStore::append).
///
/// Separates concurrency conflicts (a normal, expected condition in
/// optimistic concurrency) from adapter-level failures (I/O, connection).
/// This lets the `EventStore` facade map conflicts to
/// [`StoreError::Conflict`] without opaque wrapping.
#[derive(Debug, Error)]
pub enum AppendError<E> {
    /// Optimistic concurrency conflict — expected version doesn't match.
    #[error(
        "concurrency conflict on '{stream_id}': expected version {expected:?}, actual {actual:?}"
    )]
    Conflict {
        stream_id: ErrorId,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    /// Adapter-level failure (I/O, serialization, connection, etc.).
    #[error("store error: {0}")]
    Store(#[source] E),
}
