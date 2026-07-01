//! Embedded LSM-tree event store adapter for `nexus-store`, backed by
//! [`fjall`](https://docs.rs/fjall).
//!
//! [`FjallStore`] implements the storage traits the kernel depends on:
//! [`nexus_store::RawEventStore`] (byte-level `append` + `read_stream` +
//! `read_all`), [`nexus_store::WakeSource`](nexus_store::wake::WakeSource)
//! (the live wake the generic [`nexus_store::Subscription`] loop parks on),
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
//! `read_stream` / `read_all` return a `ScanCursor` — a concrete
//! `impl futures::Stream<Item = Result<PersistedEnvelope, FjallError>>` over
//! one lazy `fjall::Iter` (no GAT lending cursor, no bespoke combinator
//! trait). Consumers get the full [`futures::StreamExt`] / `TryStreamExt`
//! combinator surface for free. The catch-up-then-live-tail subscription loop
//! is assembled generically in `nexus_store` over `RawEventStore` +
//! [`WakeSource`](nexus_store::wake::WakeSource); fjall ships only those two
//! pieces, not a bespoke subscription cursor.
//!
//! # Wire format
//!
//! Every frame is built by [`nexus_store::wire::encode_frame`]. That helper
//! guarantees the payload bytes land on a 16-byte boundary inside the
//! `Bytes` buffer the cursor hands out — the wire-format invariant that
//! zero-copy decoders (rkyv, flatbuffers, `#[repr(C)]` POD) rely on for
//! sound `&T` reads. Encoding/decoding of the *key* (stream id + version)
//! lives in the crate-private `wire_key` module; the *value* layout is owned
//! entirely by [`nexus_store::wire`].
//!
//! [`GlobalSeq`]: crate::GlobalSeq

#![allow(
    clippy::result_large_err,
    reason = "FjallError is intentionally stack-allocated (~208 bytes) for IoT targets"
)]

mod builder;
mod error;
mod global_seq;
mod partition;
mod plan;
mod scan;
#[cfg(feature = "snapshot")]
mod snapshot;
mod store;
mod subscription_id;
mod wire_key;

pub use builder::FjallStoreBuilder;
pub use error::FjallError;
pub use global_seq::GlobalSeq;
pub use partition::{AllIndex, KeyspaceConfig};
pub use store::FjallStore;
