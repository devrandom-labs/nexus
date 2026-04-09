use crate::version::Version;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KernelError {
    #[error("Version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: Version, actual: Version },

    #[error("Rehydration limit exceeded: max {max} events")]
    RehydrationLimitExceeded { max: usize },

    #[error("Version sequence exhausted: cannot exceed u64::MAX events")]
    VersionOverflow,
}
