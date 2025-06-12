use crate::DomainEvent;
use std::{future::Future, pin::Pin};
use tower::BoxError;

pub trait EventSerializer {
    #[allow(clippy::type_complexity)]
    fn serialize<D>(
        &self,
        domain_event: D,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, BoxError>> + Send + 'static>>
    where
        D: DomainEvent;
}
