use fjall::{CompressionType, PartitionCreateOptions};

mod sealed {
    pub trait Sealed {}
}

/// Partition configuration strategy for
/// [`FjallStoreBuilder`](crate::FjallStoreBuilder).
///
/// Sealed: only `()` (use defaults) and
/// `FnOnce(PartitionCreateOptions) -> PartitionCreateOptions` closures
/// are valid implementors. Eliminates dynamic dispatch from the builder
/// by monomorphising partition configuration at compile time.
pub trait PartitionConfig: sealed::Sealed {
    /// Apply this configuration, using `defaults` as the base.
    fn apply(self, defaults: PartitionCreateOptions) -> PartitionCreateOptions;
}

impl sealed::Sealed for () {}

impl PartitionConfig for () {
    fn apply(self, defaults: PartitionCreateOptions) -> PartitionCreateOptions {
        defaults
    }
}

impl<F> sealed::Sealed for F where F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions {}

impl<F> PartitionConfig for F
where
    F: FnOnce(PartitionCreateOptions) -> PartitionCreateOptions,
{
    fn apply(self, defaults: PartitionCreateOptions) -> PartitionCreateOptions {
        self(defaults)
    }
}

/// Default options for point-read-optimised partitions (streams, snapshots,
/// checkpoints): 4 KiB blocks for small metadata lookups, 15-bit bloom
/// filter (~0.1% FPR) to minimise unnecessary I/O.
pub fn point_read_defaults() -> PartitionCreateOptions {
    PartitionCreateOptions::default()
        .block_size(4_096)
        .bloom_filter_bits(Some(15))
}

/// Default options for scan-optimised partitions (events): 32 KiB blocks
/// for efficient sequential scans, LZ4 compression for reduced disk I/O.
pub fn scan_defaults() -> PartitionCreateOptions {
    PartitionCreateOptions::default()
        .block_size(32_768)
        .compression(CompressionType::Lz4)
}
