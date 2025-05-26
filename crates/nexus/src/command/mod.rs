//! # The Command Module: Orchestrating State Changes
//!
//! This module provides the core components for the "write" side of a CQRS
//! (Command Query Responsibility Segregation) architecture, focusing on how
//! state changes are initiated, processed, and persisted, primarily through
//! the lens of Event Sourcing and Domain-Driven Design (DDD) aggregates.
//!
//! ## Key Components:
//!
//!* **`aggregate`**: Defines the `AggregateRoot`, `AggregateState`, `AggregateType` and related
//!  traits that form the heart of your event-sourced domain entities. Aggregates
//!  are consistency boundaries that process commands and produce domain events.
//!    
//!* **`handler`**: Specifies traits for command handlers, particularly
//!  `AggregateCommandHandler`, which encapsulates the logic for processing
//!  a specific command against an aggregate's state.
//!
//!     
//!* **`repository`**: Defines the `EventSourceRepository` trait, a port (in
//!  Hexagonal Architecture terms) for loading and saving event-sourced
//!  aggregates.
//!
//! By using the components in this module, you can build robust, testable, and
//! auditable systems where state mutations are explicit, event-driven, and
//! managed through well-defined domain models.
//!
use crate::DomainEvent;
use smallvec::{SmallVec, smallvec};

pub mod aggregate;
pub mod handler;
pub mod repository;

#[cfg(test)]
pub mod test;

#[derive(Debug)]
pub struct NonEmptyEvents<E, const N: usize>
where
    E: DomainEvent,
{
    first: E,
    more: SmallVec<[E; N]>,
}

impl<E, const N: usize> NonEmptyEvents<E, N>
where
    E: DomainEvent,
{
    pub fn new(event: E) -> Self {
        NonEmptyEvents {
            first: event,
            more: SmallVec::new(),
        }
    }

    pub fn add(&mut self, event: E) {
        self.more.push(event);
    }

    pub fn into_small_vec(self) -> SmallVec<[E; N]> {
        let mut events = smallvec![self.first];
        events.extend(self.more);
        events
    }
}

// TODO: create an iterator for NonEmptyEvents
// TODO: impl From trait to small_vec
// TODO: macro nonemptyevents![] to directly add to more. or first depending on the number of params in it.
