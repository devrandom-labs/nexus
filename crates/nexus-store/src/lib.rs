pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod store;
pub mod stream_label;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcasting;

#[cfg(feature = "json")]
pub use codec::serde::json::{Json, JsonCodec};
#[cfg(feature = "serde")]
pub use codec::serde::{SerdeCodec, SerdeFormat};
pub use codec::{BorrowingCodec, Codec};
pub use envelope::{PendingEnvelope, PersistedEnvelope, pending_envelope};
pub use error::{AppendError, InvalidSchemaVersion, StoreError, UpcastError};
pub use nexus::Version;
#[cfg(all(feature = "snapshot", feature = "testing"))]
pub use snapshot::InMemorySnapshotStore;
#[cfg(feature = "snapshot")]
pub use snapshot::{
    AfterEventTypes, EveryNEvents, PendingSnapshot, PersistedSnapshot, SnapshotStore,
    SnapshotTrigger,
};
pub use store::{
    EventStore, EventStream, EventStreamExt, NeedsCodec, NoSnapshot, RawEventStore, Repository,
    RepositoryBuilder, Store, ZeroCopyEventStore,
};
#[cfg(feature = "snapshot")]
pub use store::{Snapshotting, WithSnapshot};
pub use stream_label::{StreamLabel, ToStreamLabel};
pub use upcasting::{EventMorsel, Upcaster};
