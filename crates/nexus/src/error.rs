use crate::store;
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

    #[error("A storage layer operation failed")]
    Store(#[from] store::Error),

    /// ## Variant: `DeserializationError`
    /// Occurs if there's an error deserializing event data retrieved from the event store.
    /// The `source` field contains the underlying error, typically from a serialization library.
    #[error("Failed to deserialize event data from store")]
    DeserializationError {
        #[source]
        source: BoxError,
    },

    /// ## Variant: `SerializationError`
    /// Occurs if there's an error serializing event data for persistence into the event store.
    /// The `source` field contains the underlying error.
    #[error("Failed to serialize event data for store")] // Added for save
    SerializationError {
        #[source]
        source: BoxError,
    },
}
