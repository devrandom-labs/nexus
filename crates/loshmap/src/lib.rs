/// The largest possible table capacity. This value must be
const MAXIMUM_CAPACITY: usize = 1 << 30;

/// The default initial table capacity. Must be a power of 2
/// (i.e., at least 1) and at most `MAXIMUM_CAPACITY`.
const DEFAULT_CAPACITY: usize = 16;
