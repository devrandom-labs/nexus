use crate::DomainEvent;
use async_trait::async_trait;
use thiserror::Error;
use tower::BoxError;

pub mod event_record;
pub mod event_store;

pub use event_store::EventStore;

#[cfg(test)]
pub mod test;

#[async_trait]
pub trait EventSerializer {
    async fn serialize<D>(&self, domain_event: D) -> Result<Vec<u8>, BoxError>
    where
        D: DomainEvent;
}

#[async_trait]
pub trait EventDeserializer {
    async fn deserialize<D>(&self, payload: &[u8]) -> Result<D, BoxError>
    where
        D: DomainEvent;
}

#[derive(Debug, Error)]
pub enum StoreError {
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
