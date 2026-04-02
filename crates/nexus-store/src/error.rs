use nexus::ErrorId;
use nexus::Version;
use thiserror::Error;

/// Errors from the event store layer.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Optimistic concurrency conflict.
    #[error("Concurrency conflict on '{stream_id}': expected version {expected}, actual {actual}")]
    Conflict {
        stream_id: ErrorId,
        expected: Version,
        actual: Version,
    },

    /// Stream not found.
    #[error("Stream '{stream_id}' not found")]
    StreamNotFound { stream_id: ErrorId },

    /// Serialization/deserialization failure.
    #[error("Codec error: {0}")]
    Codec(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Database adapter failure.
    #[error("Adapter error: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
}
