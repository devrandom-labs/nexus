use arrayvec::ArrayString;
use thiserror::Error;

/// Errors produced by the fjall event store adapter.
///
/// Intentionally stack-allocated (~208 bytes) for IoT/embedded targets.
/// All diagnostic fields use `ArrayString` — no heap allocation on error paths.
#[derive(Debug, Error)]
pub enum FjallError {
    /// Fjall I/O or internal database error.
    #[error("fjall error: {0}")]
    Io(#[from] fjall::Error),

    /// Stored value has corrupt or unrecognizable byte layout.
    #[error("corrupt value in stream '{stream_id}' at version {version:?}")]
    CorruptValue {
        stream_id: ArrayString<64>,
        version: Option<u64>,
    },

    /// Stream metadata has wrong byte size.
    #[error("corrupt metadata for stream '{stream_id}'")]
    CorruptMeta { stream_id: ArrayString<64> },

    /// Invalid input on the write path (e.g. event type name too long).
    ///
    /// Distinct from `CorruptValue` which indicates already-persisted data
    /// is unreadable. This error fires before any data is written.
    #[error("invalid input for stream '{stream_id}' at version {version}: {reason}")]
    InvalidInput {
        stream_id: ArrayString<64>,
        version: u64,
        reason: ArrayString<128>,
    },

    /// Event version would overflow `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    /// Verify `FjallError` satisfies the bounds required by `RawEventStore::Error`.
    #[test]
    fn satisfies_error_send_sync_static() {
        fn assert_bounds<T: std::error::Error + Send + Sync + 'static>() {}
        assert_bounds::<FjallError>();
    }
}
