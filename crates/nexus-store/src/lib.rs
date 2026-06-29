//! Persistence edge layer for the nexus event-sourcing kernel.
//!
//! `nexus-store` sits between the pure-domain kernel (`nexus`) and the
//! storage adapters (`nexus-fjall`, future postgres, etc.). It owns the
//! shapes that cross the kernel↔storage boundary — envelopes, codecs,
//! event streams, repositories, and snapshot stores — and the wire-format
//! row builder every adapter is required to use.
//!
//! # Crate layout
//!
//! Flat: one file per concept, no module subdirectories. Each module's
//! own `//!` header documents its rationale.
//!
//! - [`codec`] — one [`Encode<E>`](crate::Encode) trait and one
//!   [`Decode<E>`](crate::Decode) trait with an `Output<'a>` GAT. The GAT
//!   collapses what used to be two traits (`Decode` + `BorrowingDecode`)
//!   into a single shape that covers both owning serde codecs and
//!   borrowing codecs (rkyv, bytemuck). Feature-gated codec impls
//!   (`serde`, `json`, `bytemuck`, `rkyv`) ship with the crate.
//! - [`envelope`] — [`PendingEnvelope`] (write path, typestate-built) and
//!   [`PersistedEnvelope`] (read path, owned [`bytes::Bytes`] + cached
//!   `Range<u32>` offsets). The read envelope is cheap-to-clone (Arc
//!   refcount + range copies) and has no lifetime parameter, so it flows
//!   through `futures::Stream` items without bridging code.
//! - [`store`] — adapter-facing [`RawEventStore`] trait,
//!   [`Store<S>`](crate::store::Store) shared handle, and [`GlobalSeq`]
//!   (store-wide monotonic-but-gappy stamp).
//! - [`subscription`] — user-facing [`Subscription<S>`] struct (built
//!   via `Subscription::new(&store)`). Its `subscribe` / `subscribe_all`
//!   methods assemble the generic catch-up-then-live-tail loop from
//!   [`RawEventStore`] + [`WakeSource`](crate::wake::WakeSource); there is
//!   no adapter-facing subscription trait. The returned cursor is `!Unpin`
//!   (consumers `pin!` it).
//! - [`stream`] — [`EventStream`] marker trait over
//!   `futures::Stream<Item = Result<PersistedEnvelope, _>>`. The marker
//!   carries no methods of its own — every combinator comes from
//!   [`futures::StreamExt`](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html)
//!   and [`TryStreamExt`](https://docs.rs/futures/latest/futures/stream/trait.TryStreamExt.html).
//! - [`wire`] — single canonical frame builder
//!   ([`encode_frame`](crate::wire::encode_frame)) that every adapter must use.
//!   Guarantees 16-byte payload alignment as a wire-format invariant —
//!   the precondition zero-copy decoders (rkyv, flatbuffers, `#[repr(C)]`
//!   POD) rely on for sound `&T` reads.
//! - [`repository`] / [`builder`] — aggregate-facing [`Repository<A>`]
//!   trait plus its facade impl ([`EventStore`], one terminal for both
//!   owning and borrowing codecs), constructed via the
//!   [`RepositoryBuilder`] typestate.
//! - [`state`] — [`SnapshotStore<S, P>`](crate::SnapshotStore) for atomic
//!   state+position persistence. Powers both aggregate snapshots and
//!   projection state — same trait, different position type
//!   ([`Version`] vs [`GlobalSeq`]).
//! - [`upcasting`] — schema evolution via the [`Upcaster`] trait and
//!   [`EventMorsel`] zero-copy-when-possible data unit.
//! - [`snapshot`] (feature-gated) — decorator that wraps a repository to
//!   hydrate from a [`SnapshotStore`] on read and commit on write per a
//!   [`PersistTrigger`].
//! - [`projection`] (feature-gated) — [`Projector`] trait (pure fallible
//!   fold). nexus ships no runner; the loop is consumer-owned (see
//!   `examples/projection-tokio`).
//!
//! # Feature flags
//!
//! | Feature | Effect |
//! |---|---|
//! | `serde` | Generic serde codec (`SerdeCodec<F>`) |
//! | `json` | `Json` format + `JsonCodec` alias (implies `serde`) |
//! | `bytemuck` | `BytemuckCodec` for `#[repr(C)]` POD types (zero-copy `&E`) |
//! | `rkyv` | `RkyvCodec` for rkyv-archived types (zero-copy `&Archived<E>`) |
//! | `snapshot` | `Snapshotting<R, SS, T>` repository decorator |
//! | `snapshot-json` | `snapshot` + `json` |
//! | `projection` | `Projector` trait |
//! | `projection-json` | `projection` + `json` |
//! | `subscription` | [`StreamNotifiers`](crate::notify) per-stream wake registry (pulls `tokio`, `foldhash`, `parking_lot`) |
//! | `testing` | `InMemoryStore`, `InMemorySnapshotStore` for tests (implies `subscription`) |
//!
//! # Design notes
//!
//! The earlier `codec/`, `envelope/`, `upcasting/`, `store/`,
//! `repository/`, `state/`, `projection/` directories were collapsed into
//! single files because each held only a handful of small files with no
//! cohesion benefit. The boundary that matters is the crate boundary
//! (kernel-pure → store-persistence → adapters); the boundary that
//! didn't matter was inside `nexus-store`.

pub mod batch;
pub mod builder;
#[cfg(feature = "subscription")]
pub(crate) mod catchup;
#[cfg(feature = "cbor")]
pub mod cbor;
pub mod codec;
pub mod envelope;
pub mod error;
#[cfg(feature = "export")]
pub mod export;
#[cfg(feature = "import")]
pub mod import;
#[cfg(feature = "subscription")]
pub mod notify;
#[cfg(feature = "projection")]
pub mod projection;
pub mod repository;
pub mod saga;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod state;
pub mod store;
pub mod stream;
pub mod stream_id;
#[cfg(feature = "subscription")]
pub mod subscription;
#[cfg(feature = "subscription")]
pub(crate) mod subscription_cursor;
#[cfg(feature = "testing")]
pub mod testing;
pub mod upcasting;
pub mod value;
#[cfg(feature = "subscription")]
pub mod wake;
pub mod wire;

pub use batch::{BatchSize, BatchSizeError, DEFAULT_BATCH, MAX_BATCH};
#[cfg(feature = "snapshot")]
pub use builder::WithSnapshot;
pub use builder::{NeedsCodec, NoSnapshot, RepositoryBuilder};
// Re-export `bytes` so downstreams name `nexus_store::bytes::Bytes` to feed
// `Encode` / the value newtypes, sharing *our* version rather than coupling to
// theirs. Additive (non-breaking).
pub use bytes;
#[cfg(feature = "cbor")]
pub use cbor::{
    ChunkError, ChunkHeader, ChunkWriter, SectionError, SectionWriter, WriteError, decode_chunk,
    decode_header,
};
#[cfg(feature = "json")]
pub use codec::serde::json::{Json, JsonCodec};
#[cfg(feature = "serde")]
pub use codec::serde::{SerdeCodec, SerdeFormat};
pub use codec::{Decode, Encode};
pub use envelope::{
    EnvelopeError, ForDecodeError, PendingEnvelope, PersistedEnvelope, pending_envelope,
};
pub use error::LoadWithError;
pub use error::{AppendError, StoreError};
#[cfg(feature = "export")]
pub use export::{EventExporter, StreamLister};
#[cfg(feature = "import")]
pub use import::{
    AbortReason, Atomicity, EventImporter, ImportBlock, ImportError, ImportReport, StreamOutcome,
    StreamReport, StreamSection,
};
pub use nexus::Version;
#[cfg(feature = "projection")]
pub use projection::Projector;
pub use repository::{EventStore, Repository};
pub use saga::{
    ConflictPredicate, ProjectedIntent, ProjectedIntents, ProjectedIntentsIntoIter, Reaction,
    SagaError, SagaRepository,
};
#[cfg(feature = "snapshot")]
pub use snapshot::Snapshotting;
#[cfg(feature = "testing")]
pub use state::InMemorySnapshotStore;
pub use state::{
    AfterEventTypes, CodecSnapshotStore, CodecSnapshotStoreError, EveryNEvents, PersistTrigger,
    SnapshotStore,
};
pub use store::{GlobalSeq, RawEventStore, Store};
pub use stream::EventStream;
pub use stream_id::StreamKey;
// Re-export the `Stream` trait from `futures-core` (the small, near-frozen
// definitional crate) rather than `futures`. `futures::Stream` *is* this trait,
// so our public `EventStream` / `subscribe*` surface is married to
// `futures-core`'s stability, not the churning batteries-included `futures`.
pub use futures_core::Stream;
#[cfg(feature = "subscription")]
pub use subscription::Subscription;
#[cfg(feature = "testing")]
pub use testing::InMemoryStoreError;
pub use upcasting::EventMorsel;
pub use value::{EventType, Metadata, Payload, SchemaVersion, ValueError};
#[cfg(feature = "subscription")]
pub use wake::{WakeRegistration, WakeSource};
