pub mod error;
pub mod event_store;
pub mod record;

pub use error::Error;
pub use event_store::EventStore;
pub use record::{EventRecord, StreamId};
