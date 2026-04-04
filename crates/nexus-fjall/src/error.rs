use thiserror::Error;

/// Errors produced by the fjall event store adapter.
#[derive(Debug, Error)]
pub enum FjallError {
    /// Fjall I/O or internal database error.
    #[error("fjall error: {0}")]
    Io(#[from] fjall::Error),

    /// Stored value has corrupt or unrecognizable byte layout.
    #[error("corrupt value in stream '{stream_id}' at version {version}")]
    CorruptValue { stream_id: String, version: u64 },

    /// Stream metadata has wrong byte size.
    #[error("corrupt metadata for stream '{stream_id}'")]
    CorruptMeta { stream_id: String },

    /// Stream ID must not be empty (fjall requires non-empty keys).
    #[error("stream_id must not be empty")]
    EmptyStreamId,

    /// Numeric stream ID space exhausted (`u64::MAX` streams created).
    ///
    /// This prevents silent wrap-around to 0, which would cause two
    /// different `stream_id` strings to share the same numeric key space
    /// and silently corrupt each other's event data.
    #[error(
        "stream ID space exhausted: cannot allocate more than {} streams",
        u64::MAX
    )]
    StreamIdExhausted,
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
