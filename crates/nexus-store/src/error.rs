use arrayvec::ArrayString;
use nexus::{KernelError, Version};
use thiserror::Error;

/// Errors from the event store layer.
///
/// Generic over adapter (`A`), codec (`C`), and upcaster transform (`U`)
/// error types — zero allocation, no `Box<dyn Error>`.
#[derive(Debug, Error)]
pub enum StoreError<A, C, U> {
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

    /// Serialization/deserialization failure.
    #[error("codec error: {0}")]
    Codec(#[source] C),

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
