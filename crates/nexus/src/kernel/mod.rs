pub mod aggregate;
pub mod error;
pub mod event;
pub mod events;
pub mod id;
pub mod message;
pub mod version;

pub use aggregate::{Aggregate, AggregateRoot, AggregateState, EventOf};
pub use error::KernelError;
pub use event::DomainEvent;
pub use events::Events;
pub use id::Id;
pub use message::Message;
pub use version::{Version, VersionedEvent};
