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
use crate::domain::DomainEvent;
use smallvec::{SmallVec, smallvec};

pub mod handler;
pub mod repository;

#[cfg(test)]
pub mod test;

#[derive(Debug)]
pub struct Events<E>
where
    E: DomainEvent,
{
    first: E,
    more: SmallVec<[E; 1]>,
}

impl<E> Events<E>
where
    E: DomainEvent,
{
    pub fn new(event: E) -> Self {
        Events {
            first: event,
            more: SmallVec::new(),
        }
    }

    pub fn add(&mut self, event: E) {
        self.more.push(event);
    }

    pub fn into_small_vec(self) -> SmallVec<[E; 1]> {
        let mut events = smallvec![self.first];
        events.extend(self.more);
        events
    }
}

// TODO: impl IntoIterator for this collection
// TODO: impl From trait to small_vec

#[macro_export]
macro_rules! events {
    [$head:expr] => {
        {
            let mut events = Events::new($head);
            events
        }
    };
    [$head:expr, $($tail:expr), +] => {
        {
            let mut events = Events::new($head);
            $(
                events.add($tail);
            )*
                events
        }
    }
}
