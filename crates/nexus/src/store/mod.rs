use crate::DomainEvent;
use async_trait::async_trait;
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
