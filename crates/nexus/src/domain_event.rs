use std::fmt::Debug;

pub trait DomainEvent: Debug + Send + Sync + 'static {}
