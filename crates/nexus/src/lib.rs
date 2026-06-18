mod aggregate;
mod error;
mod event;
mod events;
mod id;
mod message;
mod saga;
mod version;

#[cfg(feature = "testing")]
pub mod testing;

pub mod closing_the_books;

pub use aggregate::{
    Aggregate, AggregateRoot, AggregateState, DEFAULT_MAX_REHYDRATION_EVENTS, EventOf, Handle,
};
pub use error::KernelError;
pub use event::DomainEvent;
pub use events::Events;
pub use id::Id;
pub use message::Message;
pub use saga::{React, Saga};
pub use version::{Version, VersionedEvent};

#[cfg(feature = "derive")]
pub use nexus_macros::DomainEvent;
#[cfg(feature = "derive")]
pub use nexus_macros::aggregate;
