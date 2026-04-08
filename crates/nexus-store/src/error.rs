use crate::stream_label::StreamLabel;
use nexus::{KernelError, Version};
use thiserror::Error;

/// Errors from the event store layer.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Optimistic concurrency conflict.
    #[error(
        "Concurrency conflict on stream '{stream_id}': expected version {expected:?}, actual {actual:?}"
    )]
    Conflict {
        stream_id: StreamLabel,
        expected: Option<Version>,
        actual: Option<Version>,
    },

    /// Stream not found.
    #[error("Stream '{stream_id}' not found")]
    StreamNotFound { stream_id: StreamLabel },

    /// Serialization/deserialization failure.
    #[error("Codec error: {0}")]
    Codec(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Database adapter failure.
    #[error("Adapter error: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Kernel error during replay (e.g. version mismatch, rehydration limit).
    #[error("Kernel error: {0}")]
    Kernel(#[from] KernelError),
}

/// Structured error from [`RawEventStore::append`](crate::RawEventStore::append).
///
/// Separates concurrency conflicts (a normal, expected condition in
/// optimistic concurrency) from adapter-level failures (I/O, connection).
/// This lets the `EventStore` facade map conflicts to
/// [`StoreError::Conflict`] without opaque wrapping.
#[derive(Debug)]
pub enum AppendError<E> {
    /// Optimistic concurrency conflict — expected version doesn't match.
    Conflict {
        stream_id: StreamLabel,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    /// Adapter-level failure (I/O, serialization, connection, etc.).
    Store(E),
}

impl<E: std::fmt::Display> std::fmt::Display for AppendError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict {
                stream_id,
                expected,
                actual,
            } => write!(
                f,
                "Concurrency conflict on '{stream_id}': expected version {expected:?}, actual {actual:?}"
            ),
            Self::Store(e) => write!(f, "Store error: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for AppendError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Conflict { .. } => None,
            Self::Store(e) => Some(e),
        }
    }
}

/// Schema version 0 is invalid — schema versions start at 1.
#[derive(Debug, Error)]
#[error("invalid schema version: 0 (schema versions start at 1)")]
pub struct InvalidSchemaVersion;

/// Errors from upcaster validation and transform execution.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UpcastError {
    /// Upcaster returned the same or lower schema version.
    #[error(
        "Upcaster did not advance schema version for '{event_type}': \
         input {input_version}, output {output_version}"
    )]
    VersionNotAdvanced {
        event_type: String,
        input_version: Version,
        output_version: Version,
    },

    /// Upcaster returned an empty event type.
    #[error(
        "Upcaster returned empty event_type \
         (input: '{input_event_type}', schema version {schema_version})"
    )]
    EmptyEventType {
        input_event_type: String,
        schema_version: Version,
    },

    /// Upcaster chain exceeded the iteration limit.
    #[error(
        "Upcaster chain exceeded {limit} iterations for '{event_type}' \
         (stuck at schema version {schema_version})"
    )]
    ChainLimitExceeded {
        event_type: String,
        schema_version: Version,
        limit: u64,
    },

    /// A schema transform failed to process the payload.
    #[error("Transform failed for '{event_type}' at schema version {schema_version}: {source}")]
    TransformFailed {
        event_type: String,
        schema_version: Version,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
