use crate::{DomainEvent, error::Error as NexusError};
use async_trait::async_trait;

pub mod error;
pub mod event_record;
pub mod event_store;

pub use error::Error;
pub use event_record::{EventRecordBuilder, StreamId};
pub use event_store::EventStore;

#[cfg(test)]
pub mod test;

#[async_trait]
pub trait EventSerializer {
    async fn serialize<D>(&self, domain_event: D) -> Result<Vec<u8>, NexusError>
    where
        D: DomainEvent;
}

#[async_trait]
pub trait EventDeserializer {
    async fn deserialize<D>(&self, payload: &[u8]) -> Result<D, NexusError>
    where
        D: DomainEvent;
}
