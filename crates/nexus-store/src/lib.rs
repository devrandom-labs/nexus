pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "projection")]
pub mod projection;
pub mod repository;
pub mod state;
pub mod store;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcasting;

pub use arrayvec::ArrayString;
#[cfg(feature = "json")]
pub use codec::serde::json::{Json, JsonCodec};
#[cfg(feature = "serde")]
pub use codec::serde::{SerdeCodec, SerdeFormat};
pub use codec::{BorrowingCodec, Codec};
pub use envelope::{PendingEnvelope, PersistedEnvelope, pending_envelope};
pub use error::{AppendError, InvalidSchemaVersion, StoreError, UpcastError};
pub use nexus::Version;
#[cfg(feature = "projection")]
pub use projection::Projector;
pub use repository::{
    EventStore, NeedsCodec, NoSnapshot, Repository, RepositoryBuilder, ZeroCopyEventStore,
};
#[cfg(feature = "snapshot")]
pub use repository::{Snapshotting, WithSnapshot};
#[cfg(feature = "testing")]
pub use state::InMemoryStateStore;
pub use state::{
    AfterEventTypes, CodecStateStore, CodecStateStoreError, EveryNEvents, PersistTrigger, State,
    StateStore,
};
pub use store::{CheckpointStore, EventStream, EventStreamExt, RawEventStore, Store, Subscription};
#[cfg(feature = "testing")]
pub use testing::InMemoryStoreError;
pub use upcasting::{EventMorsel, Upcaster};
