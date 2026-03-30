use super::event::DomainEvent;
use super::id::Id;
use std::error::Error;
use std::fmt::Debug;

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
    fn name(&self) -> &'static str;
}

pub trait Aggregate {
    type State: AggregateState;
    type Error: Error + Send + Sync + Debug + 'static;
    type Id: Id;
}

pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;
