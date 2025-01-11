use crate::aggregate::Aggregate;
use crate::error::AggregateError;
use crate::event::EventEnvelope;

// a store is tied to an aggregate
// each new type of aggregate would need to have their own store,
pub trait Store<A>
where
    A: Aggregate,
{
    fn get_events(&self, id: &str) -> Result<Vec<EventEnvelope<A>>, AggregateError<A::Error>>;

    fn commit(
        &self,
        aggregate_type: &str,
        events: &[A::Event],
    ) -> Result<Vec<EventEnvelope<A>>, AggregateError<A::Error>>;
}
