use crate::aggregate::Aggregate;
use std::collections::HashMap;
use ulid::Ulid;

pub trait DomainEvent {
    fn event_type(&self) -> String;
    fn event_version(&self) -> String;
}

#[derive(Debug, Clone)]
pub struct EventEnvelope<A>
where
    A: Aggregate,
{
    pub stream_id: Ulid,
    pub payload: A::Event,
    pub metadata: HashMap<String, String>,
}
