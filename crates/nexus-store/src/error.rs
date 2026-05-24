use arrayvec::ArrayString;
use nexus::{KernelError, Version};
use thiserror::Error;

/// Errors from the event store layer.
///
/// Generic over adapter (`A`), encode (`EncErr`), decode (`DecErr`), and
/// upcaster transform (`U`) error types — zero allocation, no `Box<dyn Error>`.
///
/// `EncErr` and `DecErr` are independent so write-only and read-only codecs
/// can each set the unused side to `Infallible`. For the common case where
/// a single underlying format powers both directions, the implementor picks
/// the same `Error` associated type on both `Encode` and `Decode` impls and
/// `EncErr == DecErr` falls out without a where-clause.
#[derive(Debug, Error)]
pub enum StoreError<A, EncErr, DecErr, U> {
    /// Optimistic concurrency conflict.
    #[error(
        "concurrency conflict on stream '{stream_id}': expected version {expected:?}, actual {actual:?}"
    )]
    Conflict {
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },

    /// Stream not found.
    #[error("stream '{stream_id}' not found")]
    StreamNotFound { stream_id: ArrayString<64> },

    /// Database adapter failure.
    #[error("adapter error: {0}")]
    Adapter(#[source] A),

    /// Serialization failure on the write path.
    #[error("encode error: {0}")]
    Encode(#[source] EncErr),

    /// Deserialization failure on the read path.
    #[error("decode error: {0}")]
    Decode(#[source] DecErr),

    /// Upcaster transform failure.
    #[error("upcast error: {0}")]
    Upcast(#[source] UpcastError<U>),

    /// Kernel error during replay (e.g. version mismatch, rehydration limit).
    #[error("kernel error: {0}")]
    Kernel(#[from] KernelError),

    /// Version overflow: cannot advance past `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,
}

impl<A, EncErr, DecErr, U> From<DecodeStreamError<A, DecErr, U>>
    for StoreError<A, EncErr, DecErr, U>
{
    fn from(err: DecodeStreamError<A, DecErr, U>) -> Self {
        match err {
            DecodeStreamError::Stream(e) => Self::Adapter(e),
            DecodeStreamError::Decode(e) => Self::Decode(e),
            DecodeStreamError::Upcast(e) => Self::Upcast(e),
        }
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
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    /// Adapter-level failure (I/O, serialization, connection, etc.).
    #[error("store error: {0}")]
    Store(#[source] E),
}

/// Schema version 0 is invalid — schema versions start at 1.
#[derive(Debug, Error)]
#[error("invalid schema version: 0 (schema versions start at 1)")]
pub struct InvalidSchemaVersion;

/// Convenience error type for [`DecodedStream`](crate::stream::DecodedStream)
/// and [`BorrowedDecodedStream`](crate::stream::BorrowedDecodedStream) fold operations.
///
/// Each variant carries the underlying error from one stage of the read
/// pipeline. Generic over the three error sources so callers may use it
/// directly or define their own enum with `From` impls for the same three
/// errors.
#[derive(Debug, Error)]
pub enum DecodeStreamError<S, DecErr, U> {
    /// Underlying stream failure.
    #[error("stream error: {0}")]
    Stream(#[source] S),

    /// Decode failure.
    #[error("decode error: {0}")]
    Decode(#[source] DecErr),

    /// Upcaster transform failure.
    #[error("upcaster error: {0}")]
    Upcast(#[source] UpcastError<U>),
}

/// Errors from upcaster validation and transform execution.
///
/// Generic over the transform error type `U` — each `Upcaster`
/// implementation specifies its own error type via `Upcaster::Error`.
#[derive(Debug, Error)]
pub enum UpcastError<U> {
    /// Upcaster returned the same or lower schema version.
    #[error(
        "upcaster did not advance schema version for '{event_type}': input {input_version}, output {output_version}"
    )]
    VersionNotAdvanced {
        event_type: ArrayString<64>,
        input_version: Version,
        output_version: Version,
    },

    /// Upcaster returned an empty event type.
    #[error(
        "upcaster returned empty event_type (input: '{input_event_type}', schema version {schema_version})"
    )]
    EmptyEventType {
        input_event_type: ArrayString<64>,
        schema_version: Version,
    },

    /// Upcaster chain exceeded the iteration limit.
    #[error(
        "upcaster chain exceeded {limit} iterations for '{event_type}' (stuck at schema version {schema_version})"
    )]
    ChainLimitExceeded {
        event_type: ArrayString<64>,
        schema_version: Version,
        limit: u64,
    },

    /// A schema transform failed to process the payload.
    #[error("transform failed for '{event_type}' at schema version {schema_version}: {source}")]
    TransformFailed {
        event_type: ArrayString<64>,
        schema_version: Version,
        #[source]
        source: U,
    },
}
