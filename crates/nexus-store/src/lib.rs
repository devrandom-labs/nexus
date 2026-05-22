pub mod builder;
pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "projection")]
pub mod projection;
pub mod repository;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod state;
pub mod store;
pub mod stream;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcasting;

pub use arrayvec::ArrayString;
#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
pub use builder::{NeedsCodec, NoSnapshot, RepositoryBuilder};
#[cfg(feature = "json")]
pub use codec::serde::json::{Json, JsonCodec};
#[cfg(feature = "serde")]
pub use codec::serde::{SerdeCodec, SerdeFormat};
pub use codec::{BorrowingCodec, Codec};
pub use envelope::{PendingEnvelope, PersistedEnvelope, pending_envelope};
pub use error::{AppendError, DecodeStreamError, InvalidSchemaVersion, StoreError, UpcastError};
pub use nexus::Version;
#[cfg(feature = "projection")]
pub use projection::Projector;
pub use repository::{EventStore, Repository, ZeroCopyEventStore};
#[cfg(feature = "snapshot")]
pub use snapshot::Snapshotting;
#[cfg(feature = "testing")]
pub use state::InMemorySnapshotStore;
pub use state::{
    AfterEventTypes, CodecSnapshotStore, CodecSnapshotStoreError, EveryNEvents, PersistTrigger,
    SnapshotStore,
};
pub use store::{GlobalSeq, RawEventStore, Store, Subscription};
pub use stream::{
    BorrowedDecodedStream, DecodedStream, DecoderBuilder, Disposition, EventStream, EventStreamExt,
};
#[cfg(feature = "testing")]
pub use testing::InMemoryStoreError;
pub use upcasting::{EventMorsel, Upcaster};
