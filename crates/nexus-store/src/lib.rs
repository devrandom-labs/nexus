pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "projection")]
pub mod projection;
#[cfg(feature = "snapshot")]
pub mod snapshot;
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
pub use projection::{
    AfterEventTypes as ProjAfterEventTypes, EveryNEvents as ProjEveryNEvents, PendingState,
    PersistedState, ProjectionTrigger, StateStore,
};
#[cfg(all(feature = "snapshot", feature = "testing"))]
pub use snapshot::InMemorySnapshotStore;
#[cfg(feature = "snapshot")]
pub use snapshot::{
    AfterEventTypes, EveryNEvents, PendingSnapshot, PersistedSnapshot, SnapshotStore,
    SnapshotTrigger,
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
