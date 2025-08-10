pub mod builder;
pub mod metadata;
pub mod pending;
pub mod persisted;
pub mod versioned_event;

pub use metadata::EventMetadata;
pub use pending::PendingEvent;
pub use persisted::PersistedEvent;
pub use versioned_event::VersionedEvent;

use crate::domain::DomainEvent;

pub type BoxedEvent = Box<dyn DomainEvent>;
