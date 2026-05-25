use arrayvec::ArrayString;
use nexus::{KernelError, Version};
use thiserror::Error;

/// Errors from the event store layer.
///
/// Generic over adapter (`A`), encode (`EncErr`), decode (`DecErr`), and
/// upcaster transform (`UpErr`) error types — zero allocation, no `Box<dyn Error>`.
///
/// `EncErr` and `DecErr` are independent so write-only and read-only codecs
/// can each set the unused side to `Infallible`. For the common case where
/// a single underlying format powers both directions, the implementor picks
/// the same `Error` associated type on both `Encode` and `Decode` impls and
/// `EncErr == DecErr` falls out without a where-clause.
#[derive(Debug, Error)]
pub enum StoreError<A, EncErr, DecErr, UpErr> {
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
    ///
    /// Carries the upcaster's raw error type (the `Self::Error` associated type
    /// from the [`Upcaster`](crate::Upcaster) implementation). No wrapper is
    /// applied; if the user wants diagnostic context (event type, schema
    /// version), they should encode it in their own error type.
    #[error("upcast error: {0}")]
    Upcast(#[source] UpErr),

    /// Kernel error during replay (e.g. version mismatch, rehydration limit).
    #[error("kernel error: {0}")]
    Kernel(#[from] KernelError),

    /// Version overflow: cannot advance past `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,
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
