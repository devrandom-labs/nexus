use crate::aggregate::Aggregate;
use std::collections::HashMap;

pub trait DomainEvent {
    fn event_type(&self) -> String;
    fn event_version(&self) -> String;
}

#[derive(Debug, Clone)]
pub struct EventEnvelope<A>
where
    A: Aggregate,
{
    pub aggregate_id: String,
    pub stream_id: String,
    pub payload: A::Event,
    pub metadata: HashMap<String, String>,
}
