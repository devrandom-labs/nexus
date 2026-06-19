//! Embedded LSM-tree event store adapter for `nexus-store`, backed by
//! [`fjall`](https://docs.rs/fjall).
//!
//! [`FjallStore`] implements the three storage traits the kernel depends
//! on: [`nexus_store::RawEventStore`] (byte-level `append` +
//! `read_stream`), [`nexus_store::Subscription`] (catch-up + live tail),
//! and — under the `snapshot` feature —
//! [`nexus_store::SnapshotStore<Vec<u8>, Version>`].
//!
//! # Partitions
//!
//! - `streams` — `id_bytes → version counter`. Point-read optimized.
//! - `events` — event rows. Scan-optimized, LZ4 compressed.
//! - `global` — one key holding the store-wide [`GlobalSeq`] counter.
//! - `snapshots` (under `snapshot` feature) — `id_bytes → snapshot blob`.
//!
//! Every write goes through one atomic `fjall::write_tx`. `append`
//! claims its [`GlobalSeq`] range inside the same transaction that
//! writes the event rows and the new version counter — half-writes are
//! unrepresentable.
//!
//! # Why fjall's `bytes_1` feature is load-bearing
//!
//! The workspace pins `fjall = { version = "3", features = ["bytes_1"] }`.
//! That feature flips fjall's `Slice` value type to be a newtype over
//! [`bytes::Bytes`] with zero-copy `From<Bytes>` and
//! `From<Slice> for Bytes` conversions. Without it, every fjall read on
//! the hot path would pay one alloc + memcpy to materialize a `Bytes`
//! envelope. With it, the [`bytes::Bytes`] in
//! [`nexus_store::PersistedEnvelope`] is the same Arc-counted buffer
//! fjall handed us — zero copy from disk LSM block to the envelope's
//! payload bytes.
//!
//! # Read path is `futures::Stream`
//!
//! [`FjallStream`] and [`FjallSubscriptionStream`] are concrete
//! `impl futures::Stream<Item = Result<PersistedEnvelope, FjallError>>`
//! types — no GAT lending cursor, no bespoke combinator trait. Consumers
//! get the full [`futures::StreamExt`] / `TryStreamExt` combinator
//! surface for free. The subscription stream uses
//! [`futures::stream::unfold`] for its notify/refill loop and re-reads
//! the underlying store when caught up (rather than terminating) so
//! `from: None` = beginning and `from: Some(v)` = strictly-after `v`.
//!
//! # Wire format
//!
//! Every frame is built by [`nexus_store::wire::encode_frame`]. That helper
//! guarantees the payload bytes land on a 16-byte boundary inside the
//! `Bytes` buffer the cursor hands out — the wire-format invariant that
//! zero-copy decoders (rkyv, flatbuffers, `#[repr(C)]` POD) rely on for
//! sound `&T` reads. Encoding/decoding of the *key* (stream id + version)
//! lives in [`encoding`]; the *value* layout is owned entirely by
//! [`nexus_store::wire`].
//!
//! [`GlobalSeq`]: nexus_store::store::GlobalSeq

#![allow(
    clippy::result_large_err,
    reason = "FjallError is intentionally stack-allocated (~208 bytes) for IoT targets"
)]

pub mod all_stream;
pub mod builder;
pub mod encoding;
pub mod error;
mod partition;
pub mod store;
pub mod stream;
mod subscription_stream;

pub use all_stream::FjallAllStream;
pub use builder::FjallStoreBuilder;
pub use error::FjallError;
pub use partition::KeyspaceConfig;
pub use store::FjallStore;
pub use stream::FjallStream;
pub use subscription_stream::FjallSubscriptionStream;
