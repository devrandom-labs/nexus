use nexus::ErrorId;
use nexus_store::envelope::EnvelopeError;

/// Errors produced by the postgres event store adapter.
///
/// Distinct failure domains (CLAUDE rule 3); diagnostic fields use
/// [`ErrorId`] to stay allocation-light on error paths.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PostgresError {
    /// A `sqlx`/postgres I/O, connection, or query error.
    #[error("postgres error: {0}")]
    Sqlx(#[source] sqlx::Error),

    /// A persisted row decoded into bytes that fail envelope validation.
    #[error("envelope integrity error in stream '{stream_id}' at version {version}")]
    EnvelopeCorrupt {
        stream_id: ErrorId,
        version: u64,
        #[source]
        source: EnvelopeError,
    },

    /// A stored row has an out-of-range / corrupt scalar (e.g. `global_seq` <= 0,
    /// version <= 0, `schema_version` == 0).
    #[error("corrupt row in stream '{stream_id}': {reason}")]
    CorruptRow {
        stream_id: ErrorId,
        reason: ErrorId<128>,
    },

    /// Building the canonical wire frame for a row failed.
    #[error("wire frame build failed in stream '{stream_id}' at version {version}: {reason}")]
    Frame {
        stream_id: ErrorId,
        version: u64,
        reason: ErrorId<128>,
    },

    /// Wake registration over `LISTEN/NOTIFY` failed.
    #[error("listen/notify wake setup failed: {0}")]
    Wake(#[source] sqlx::Error),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    /// Verify `PostgresError` satisfies the bounds required by `RawEventStore::Error`.
    #[test]
    fn satisfies_error_send_sync_static() {
        fn assert_bounds<T: std::error::Error + Send + Sync + 'static>() {}
        assert_bounds::<PostgresError>();
    }

    /// Defensive boundary: an over-long stream id in the `ErrorId<64>` field
    /// renders truncated and `…`-suffixed through the error's `Display`.
    #[test]
    fn corrupt_row_display_truncates_overlong_stream_id() {
        let err = PostgresError::CorruptRow {
            stream_id: ErrorId::from_display(&"s".repeat(200)),
            reason: ErrorId::from_display(&"bad version"),
        };
        let msg = err.to_string();
        assert!(msg.contains('…'), "display must signal truncation: {msg}");
        assert!(!msg.contains(&"s".repeat(100)));
    }
}
