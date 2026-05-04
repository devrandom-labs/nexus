pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "projection")]
pub mod projection;
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
#[cfg(feature = "testing")]
pub use state::InMemoryStateStore;
pub use state::{
    AfterEventTypes, CodecStateStore, CodecStateStoreError, EveryNEvents, PendingState,
    PersistTrigger, PersistedState, StateStore,
};
pub use store::{
    CheckpointStore, EventStore, EventStream, EventStreamExt, NeedsCodec, NoSnapshot,
    RawEventStore, Repository, RepositoryBuilder, Store, Subscription, ZeroCopyEventStore,
};
#[cfg(feature = "snapshot")]
pub use store::{Snapshotting, WithSnapshot};
#[cfg(feature = "testing")]
pub use testing::InMemoryStoreError;
pub use upcasting::{EventMorsel, Upcaster};
