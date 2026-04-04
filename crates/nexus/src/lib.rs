mod aggregate;
mod error;
mod event;
mod events;
mod id;
mod message;
mod stream_id;
mod version;

pub use aggregate::{
    Aggregate, AggregateEntity, AggregateRoot, AggregateState, DEFAULT_MAX_REHYDRATION_EVENTS,
    DEFAULT_MAX_UNCOMMITTED, EventOf,
};
pub use error::{ErrorId, KernelError};
pub use event::DomainEvent;
pub use events::Events;
pub use id::Id;
pub use message::Message;
pub use stream_id::{InvalidStreamId, MAX_STREAM_ID_LEN, StreamId};
pub use version::{Version, VersionedEvent};

#[cfg(feature = "derive")]
pub use nexus_macros::DomainEvent;
#[cfg(feature = "derive")]
pub use nexus_macros::aggregate;
