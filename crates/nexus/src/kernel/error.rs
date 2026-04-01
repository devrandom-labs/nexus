use super::version::Version;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("Version mismatch on '{stream_id}': expected {expected}, got {actual}")]
    VersionMismatch {
        stream_id: String,
        expected: Version,
        actual: Version,
    },
}
