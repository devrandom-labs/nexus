pub mod borrowing_codec;
pub mod codec;
pub mod envelope;
pub mod error;
pub mod event_store;
pub mod raw;
pub mod repository;
pub mod stream;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcaster;
pub mod upcaster_chain;

pub use borrowing_codec::BorrowingCodec;
pub use codec::Codec;
pub use envelope::{PendingEnvelope, PersistedEnvelope, pending_envelope};
pub use error::{AppendError, InvalidSchemaVersion, StoreError, UpcastError};
pub use event_store::{EventStore, ZeroCopyEventStore};
pub use nexus::StreamId;
pub use raw::RawEventStore;
pub use repository::Repository;
pub use stream::EventStream;
pub use upcaster::{EventUpcaster, apply_upcasters};
pub use upcaster_chain::{Chain, UpcasterChain};
