use crate::version::Version;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("Version mismatch on '{stream_id}': expected {expected}, got {actual}")]
    VersionMismatch {
        stream_id: String,
        expected: Version,
        actual: Version,
    },

    #[error("Rehydration limit exceeded on '{stream_id}': max {max} events")]
    RehydrationLimitExceeded {
        stream_id: String,
        max: usize,
    },
}
