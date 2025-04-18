use crate::DomainEvent;
use std::{fmt::Debug, hash::Hash};

pub mod aggregate_root;
pub mod command_handler;

pub use aggregate_root::AggregateRoot;
pub use command_handler::AggregateCommandHandler;

/// Represents the actual state data of an aggregate.
/// It's rebuilt by applying events and must know how to apply them.
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    /// Specific type of Domain Event this state reacts to.
    type Event: DomainEvent;

    /// Mutates the state based on a receieved event.
    /// This is the core of state reconstruction in Event Sourcing.
    /// It should *NOT* contain complex logic or validation. just state mutation.
    fn apply(&mut self, event: &Self::Event);
}

/// Marker trait to define the component types of a specific Aggregate kind.
/// Implement this on a distinct type (often a unit struct) for each aggregate.
pub trait AggregateType: Send + Sync + Debug + Copy + Clone + 'static {
    /// The type used to uniquely identify this aggregate instance.
    type Id: Send + Sync + Debug + Eq + Hash + Clone + 'static;
    /// The specific type of Domain Event associated with this aggregate.
    type Event: DomainEvent + PartialEq;
    /// The specific type representing the internal state data.
    /// Crucially links the State's Event type to the AggregateType's Event type.
    type State: AggregateState<Event = Self::Event>;
}

/// Provides a standard interface for accessing aggregate properties.
/// Implemented by `AggregateRoot`.
pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Send + Sync + Debug + Eq + Hash + Clone + 'static;
    type Event: DomainEvent + PartialEq;
    type State: AggregateState<Event = Self::Event>;

    fn id(&self) -> &Self::Id;
    /// Returns the version loaded from the store (basis for concurrency checks).
    fn version(&self) -> u64;
    fn state(&self) -> &Self::State;
    /// Takes ownership of newly generated events for saving.
    fn take_uncommitted_events(&mut self) -> Vec<Self::Event>;
}
