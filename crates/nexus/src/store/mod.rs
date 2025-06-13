use crate::DomainEvent;
use std::{future::Future, pin::Pin};
use tower::BoxError;

pub mod event_record;
pub mod event_store;

#[cfg(test)]
pub mod test;

pub trait EventSerializer {
    #[allow(clippy::type_complexity)]
    fn serialize<D>(
        &self,
        domain_event: D,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, BoxError>> + Send + 'static>>
    where
        D: DomainEvent;
}

pub trait EventDeserializer {
    #[allow(clippy::type_complexity)]
    fn deserialize<D>(
        &self,
        payload: &[u8],
    ) -> Pin<Box<dyn Future<Output = Result<D, BoxError>> + Send + 'static>>
    where
        D: DomainEvent;
}
