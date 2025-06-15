use thiserror::Error as Err;
use tower::BoxError;

#[derive(Debug, Err)]
pub enum Error {
    #[error("A connection to the data store could not be established")]
    ConnectionFailed {
        #[source]
        source: BoxError,
    },

    #[error("Source '{name}' not found (e.g., Table, Collection, Document)")]
    SourceNotFound { name: String },

    #[error("Stream with ID '{id}' not found")]
    StreamNotFound { id: String },

    #[error("A stream with ID '{id}' already exists, violating a unique constraint")]
    UniqueIdViolation { id: String },

    #[error(transparent)]
    Underlying {
        #[from]
        source: BoxError,
    },
}
