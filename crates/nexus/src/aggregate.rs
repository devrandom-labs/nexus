use super::DomainEvent;
use std::fmt::Debug;

/// Represents the actual state data of an aggregate.
/// It's rebuilt by applying events and must know how to apply them.
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    /// Specific type of Domain Event this state reacts to.
    type Event: DomainEvent;

    /// Mutates the state based on a receieved event.
    /// This is the core of state reconstruction in Event Sourcing.
    /// It should *NOT* contain complex logic or validation. just state mutation.
    fn apply(&mut self, event: Self::Event);
}
