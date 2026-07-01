//! Everything about the store's partitions: how they are **configured**
//! ([`KeyspaceConfig`] + the `*_defaults`) and, once opened, how their on-disk
//! **layout** is read and written ([`Partitions`]).

use fjall::config::{
    BlockSizePolicy, BloomConstructionPolicy, CompressionPolicy, FilterPolicy, FilterPolicyEntry,
};
use fjall::{
    CompressionType, KeyspaceCreateOptions, Readable, SingleWriterTxKeyspace, SingleWriterWriteTx,
    Slice,
};
use nexus::ErrorId;
use nexus_store::StreamKey;

use crate::error::FjallError;
use crate::plan::StagedRow;
use crate::wire_key::{decode_stream_version, encode_stream_version};

mod sealed {
    pub trait Sealed {}
}

/// Keyspace configuration strategy for
/// [`FjallStoreBuilder`](crate::FjallStoreBuilder).
///
/// Sealed: only `()` (use defaults) and
/// `FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions` closures
/// are valid implementors. Eliminates dynamic dispatch from the builder
/// by monomorphising keyspace configuration at compile time.
pub trait KeyspaceConfig: sealed::Sealed {
    /// Apply this configuration, using `defaults` as the base.
    fn apply(self, defaults: KeyspaceCreateOptions) -> KeyspaceCreateOptions;
}

impl sealed::Sealed for () {}

impl KeyspaceConfig for () {
    fn apply(self, defaults: KeyspaceCreateOptions) -> KeyspaceCreateOptions {
        defaults
    }
}

impl<F> sealed::Sealed for F where F: FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions {}

impl<F> KeyspaceConfig for F
where
    F: FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions,
{
    fn apply(self, defaults: KeyspaceCreateOptions) -> KeyspaceCreateOptions {
        self(defaults)
    }
}

/// Default options for point-read-optimised keyspaces (streams, snapshots,
/// checkpoints): 4 KiB blocks for small metadata lookups, bloom filter
/// with 15 bits per key (~0.003% FPR) to minimise unnecessary I/O.
pub fn point_read_defaults() -> KeyspaceCreateOptions {
    KeyspaceCreateOptions::default()
        .data_block_size_policy(BlockSizePolicy::all(4_096))
        .filter_policy(FilterPolicy::all(FilterPolicyEntry::Bloom(
            BloomConstructionPolicy::BitsPerKey(15.0),
        )))
        .expect_point_read_hits(true)
}

/// Default options for scan-optimised keyspaces (events): 32 KiB blocks
/// for efficient sequential scans, LZ4 compression for reduced disk I/O.
pub fn scan_defaults() -> KeyspaceCreateOptions {
    KeyspaceCreateOptions::default()
        .data_block_size_policy(BlockSizePolicy::all(32_768))
        .data_block_compression_policy(CompressionPolicy::all(CompressionType::Lz4))
}

/// The single key under which the store-global sequence counter is kept in the
/// `global` partition.
const GLOBAL_SEQ_KEY: &[u8] = b"global_seq";

/// Whether the store maintains the `$all` cross-stream index (the
/// `events_global` partition).
///
/// The `$all` index is a **denormalization**: `append` writes a second full copy
/// of every event's frame keyed by global sequence, so `read_all` is a single
/// sequential scan with no per-row lookup (the read-optimized, `EventStoreDB`-style
/// default). Measured, that copy adds ~27% (120-byte payloads) up to ~2× (large
/// payloads) to on-disk / journal write volume.
///
/// On a **produce-and-sync `IoT` device** — one that appends per-stream and exports
/// upward but never reads `$all` locally — that copy is pure write/storage
/// overhead the device pays but never uses (flash wear + space are the binding
/// constraints there). [`Disabled`](Self::Disabled) skips it entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AllIndex {
    /// Maintain the read-optimized `$all` index (default). `read_all` is a single
    /// sequential scan. Costs the frame copy on every `append`.
    #[default]
    Denormalized,
    /// Do **not** maintain the `$all` index: `append` skips the `events_global`
    /// write (reclaiming the copy) and `read_all` returns
    /// [`FjallError::AllIndexDisabled`](crate::FjallError::AllIndexDisabled). For
    /// produce-and-sync devices that never read `$all` locally. Opt-in and
    /// explicit — a store built this way advertises no working `$all`.
    Disabled,
}

/// The opened fjall keyspaces plus the codecs that read and write them — the
/// crate's **one** owner of the physical layout.
///
/// Every read and write of the `streams` / `events` / `events_global` / `global`
/// (and, under the `snapshot` feature, `snapshots`) partitions goes through a
/// method here, so the rest of the crate — `append`, the atomic-append path, the
/// snapshot store, the export lister — never names a partition or a key format.
///
/// The `$all` **denormalization** lives in exactly one method,
/// [`stage_event`](Self::stage_event): a single event's frame is written under
/// both its per-stream key (`events`) and its global-sequence key
/// (`events_global`). Making that the only site of the dual-write is what lets
/// the denormalization decision be revisited in one place rather than kept in
/// lock-step across two append paths.
pub struct Partitions {
    streams: SingleWriterTxKeyspace,
    events: SingleWriterTxKeyspace,
    events_global: SingleWriterTxKeyspace,
    global: SingleWriterTxKeyspace,
    #[cfg(feature = "snapshot")]
    snapshots: SingleWriterTxKeyspace,
    /// Whether the `$all` index (`events_global`) is maintained — gates the
    /// second write in [`stage_event`](Self::stage_event) and `read_all`.
    mode: AllIndex,
}

impl Partitions {
    /// Assemble from the keyspaces the builder opened, with the default
    /// (`Denormalized`) `$all` index. Use [`with_all_index`](Self::with_all_index)
    /// to select a different mode.
    pub const fn new(
        streams: SingleWriterTxKeyspace,
        events: SingleWriterTxKeyspace,
        events_global: SingleWriterTxKeyspace,
        global: SingleWriterTxKeyspace,
        #[cfg(feature = "snapshot")] snapshots: SingleWriterTxKeyspace,
    ) -> Self {
        Self {
            streams,
            events,
            events_global,
            global,
            #[cfg(feature = "snapshot")]
            snapshots,
            mode: AllIndex::Denormalized,
        }
    }

    /// Set the `$all` index mode (builder-style; consumes and returns `self`).
    #[must_use]
    pub const fn with_all_index(mut self, mode: AllIndex) -> Self {
        self.mode = mode;
        self
    }

    // ----- write-transaction reads --------------------------------------

    /// Point-read the current version counter for `id` within `tx`. Returns `0`
    /// for a stream that does not yet exist; a wrong-sized value is corruption.
    pub fn read_version(
        &self,
        tx: &SingleWriterWriteTx<'_>,
        id: &StreamKey,
    ) -> Result<u64, FjallError> {
        tx.get(&self.streams, id.as_ref())
            .map_err(FjallError::Io)?
            .map_or(Ok(0), |version_bytes| {
                decode_stream_version(&version_bytes).map_err(|_| FjallError::CorruptMeta {
                    stream_id: ErrorId::from_display(id),
                })
            })
    }

    /// Point-read the store-global sequence counter within `tx`. Absent = `0`.
    /// `id` only labels a corrupt-meta error.
    pub fn read_global(
        &self,
        tx: &SingleWriterWriteTx<'_>,
        id: &StreamKey,
    ) -> Result<u64, FjallError> {
        tx.get(&self.global, GLOBAL_SEQ_KEY)
            .map_err(FjallError::Io)?
            .map_or(Ok(0), |bytes| {
                let raw: [u8; 8] =
                    bytes
                        .as_ref()
                        .try_into()
                        .map_err(|_| FjallError::CorruptMeta {
                            stream_id: ErrorId::from_display(id),
                        })?;
                Ok(u64::from_le_bytes(raw))
            })
    }

    // ----- write-transaction writes -------------------------------------

    /// Stage one event into the per-stream `events` index, and — when the `$all`
    /// index is [`Denormalized`](AllIndex::Denormalized) — a second full copy
    /// into the `events_global` index within `tx`. **This is the only site of the
    /// `$all` denormalization.**
    ///
    /// The second write is the denormalization that makes `read_all` a
    /// point-read-free sequential scan (see [`AllIndex`]); under
    /// [`AllIndex::Disabled`] it is skipped, reclaiming the ~27%–2× write/storage
    /// the copy costs, at the price of no working `$all` on this store.
    pub fn stage_event(&self, tx: &mut SingleWriterWriteTx<'_>, row: &StagedRow) {
        let slice = Slice::from(row.frame.clone());
        tx.insert(&self.events, &row.event_key, slice.clone());
        if self.mode == AllIndex::Denormalized {
            tx.insert(&self.events_global, row.global_key, slice);
        }
    }

    /// The configured `$all` index mode. `read_all` consults this to reject
    /// reads on a store built with [`AllIndex::Disabled`].
    pub const fn all_index(&self) -> AllIndex {
        self.mode
    }

    /// Advance the stream version counter for `id` within `tx`.
    pub fn set_version(&self, tx: &mut SingleWriterWriteTx<'_>, id: &[u8], version: u64) {
        tx.insert(&self.streams, id, encode_stream_version(version));
    }

    /// Advance the store-global sequence counter within `tx`.
    pub fn set_global(&self, tx: &mut SingleWriterWriteTx<'_>, global: u64) {
        tx.insert(&self.global, GLOBAL_SEQ_KEY, global.to_le_bytes());
    }

    // ----- read-path keyspace access ------------------------------------

    /// The `events` keyspace, for opening a per-stream bounded scan.
    pub const fn events(&self) -> &SingleWriterTxKeyspace {
        &self.events
    }

    /// The `events_global` keyspace, for opening a `$all` bounded scan.
    pub const fn events_global(&self) -> &SingleWriterTxKeyspace {
        &self.events_global
    }

    /// A lazy, snapshot-pinned iterator over the `streams` partition's keys —
    /// one key per stream id — for the export lister.
    #[cfg(feature = "export")]
    pub fn stream_ids(&self) -> fjall::Iter {
        self.streams.inner().iter()
    }

    // ----- snapshots (best-effort, outside the event tx) ----------------

    /// Point-read a snapshot blob by id.
    #[cfg(feature = "snapshot")]
    pub fn read_snapshot(&self, id: &[u8]) -> Result<Option<Slice>, FjallError> {
        self.snapshots.get(id).map_err(FjallError::Io)
    }

    /// Write a snapshot blob for id (best-effort, non-transactional).
    #[cfg(feature = "snapshot")]
    pub fn write_snapshot(&self, id: &[u8], bytes: &[u8]) -> Result<(), FjallError> {
        self.snapshots
            .insert(id, Slice::from(bytes))
            .map_err(FjallError::Io)
    }

    // ----- white-box test access ----------------------------------------

    /// The `streams` keyspace. `#[cfg(test)]` — white-box tests inspect the
    /// version-counter partition directly.
    #[cfg(test)]
    pub const fn streams(&self) -> &SingleWriterTxKeyspace {
        &self.streams
    }

    /// The `global` keyspace. `#[cfg(test)]` — white-box tests inspect the
    /// counter partition directly.
    #[cfg(test)]
    pub const fn global(&self) -> &SingleWriterTxKeyspace {
        &self.global
    }
}
