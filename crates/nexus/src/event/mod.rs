pub mod builder;
pub mod metadata;
pub mod pending;
pub mod persisted;

pub use metadata::EventMetadata;
pub use pending::PendingEvent;
pub use persisted::PersistedEvent;

use crate::domain::DomainEvent;

pub type BoxedEvent = Box<dyn DomainEvent>;
