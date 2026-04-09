pub mod codec;
pub mod envelope;
pub mod error;
pub mod store;
pub mod stream_label;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcasting;

pub use codec::{BorrowingCodec, Codec};
pub use envelope::{PendingEnvelope, PersistedEnvelope, pending_envelope};
pub use error::{AppendError, InvalidSchemaVersion, StoreError, UpcastError};
pub use nexus::Version;
pub use store::{EventStore, EventStream, RawEventStore, Repository, Store, ZeroCopyEventStore};
pub use stream_label::{StreamLabel, ToStreamLabel};
pub use upcasting::{EventMorsel, Upcaster};
