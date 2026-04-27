use fjall::config::{
    BlockSizePolicy, BloomConstructionPolicy, CompressionPolicy, FilterPolicy, FilterPolicyEntry,
};
use fjall::{CompressionType, KeyspaceCreateOptions};

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
