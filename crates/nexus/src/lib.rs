mod aggregate;
mod error;
mod event;
mod events;
mod id;
mod message;
mod version;

pub use aggregate::{Aggregate, AggregateEntity, AggregateRoot, AggregateState, EventOf};
pub use error::KernelError;
pub use event::DomainEvent;
pub use events::Events;
pub use id::Id;
pub use message::Message;
pub use version::{Version, VersionedEvent};

#[cfg(feature = "derive")]
pub use nexus_macros::DomainEvent;
