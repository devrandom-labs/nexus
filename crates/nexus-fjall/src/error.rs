use nexus::ErrorId;
use nexus_store::envelope::EnvelopeError;
use thiserror::Error;

/// Format a `Display` value into a stack-allocated reason label, truncating
/// at 128 bytes on a char boundary with a trailing `…` if it overflows
/// (truncation is visually signalled, per CLAUDE.md).
pub fn reason_label(value: &impl std::fmt::Display) -> ErrorId<128> {
    ErrorId::from_display(value)
}

/// Errors produced by the fjall event store adapter.
///
/// Intentionally stack-allocated (~208 bytes) for IoT/embedded targets.
/// All diagnostic fields use [`ErrorId`] — no heap allocation on error paths.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FjallError {
    /// Fjall I/O or internal database error.
    #[error("fjall error: {0}")]
    Io(#[from] fjall::Error),

    /// Stored value has corrupt or unrecognizable byte layout.
    #[error("corrupt value in stream '{stream_id}' at version {version:?}")]
    CorruptValue {
        stream_id: ErrorId,
        version: Option<u64>,
    },

    /// Persisted envelope failed integrity validation on read.
    ///
    /// Distinguishes envelope-level corruption (UTF-8, ranges, `schema_version`)
    /// from wire-format-level corruption (`CorruptValue`).
    #[error("envelope integrity error in stream '{stream_id}' at version {version}")]
    EnvelopeCorrupt {
        stream_id: ErrorId,
        version: u64,
        #[source]
        source: EnvelopeError,
    },

    /// Stream metadata has wrong byte size.
    #[error("corrupt metadata for stream '{stream_id}'")]
    CorruptMeta { stream_id: ErrorId },

    /// Invalid input on the write path (e.g. event type name too long).
    ///
    /// Distinct from `CorruptValue` which indicates already-persisted data
    /// is unreadable. This error fires before any data is written.
    #[error("invalid input for stream '{stream_id}' at version {version}: {reason}")]
    InvalidInput {
        stream_id: ErrorId,
        version: u64,
        reason: ErrorId<128>,
    },

    /// Event version would overflow `u64::MAX`.
    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,

    /// The store-global sequence counter would overflow `u64::MAX`.
    #[error("global sequence overflow: cannot advance past u64::MAX")]
    GlobalSeqOverflow,

    /// `read_all` was called on a store built with
    /// [`AllIndex::Disabled`](crate::AllIndex): it maintains no `$all` index, so
    /// there is nothing to read. Append + per-stream reads still work; this is
    /// the deliberate produce-and-sync configuration, not a failure.
    #[error("the $all index is disabled on this store (AllIndex::Disabled)")]
    AllIndexDisabled,

    /// Failed to register a per-stream subscription wake handle.
    ///
    /// Surfaces [`NotifyError`](nexus_store::notify::NotifyError) from the
    /// wake registry — in practice only the unreachable subscriber-count
    /// overflow.
    #[error("subscription wake registration failed")]
    Subscription(#[from] nexus_store::notify::NotifyError),
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

    /// Defensive boundary: an over-long stream id in the `ErrorId<64>` field
    /// renders truncated and `…`-suffixed through the error's `Display`.
    #[test]
    fn corrupt_value_display_truncates_overlong_stream_id() {
        let err = FjallError::CorruptValue {
            stream_id: ErrorId::from_display(&"s".repeat(200)),
            version: Some(7),
        };
        let msg = err.to_string();
        assert!(msg.contains('…'), "display must signal truncation: {msg}");
        // The 64-byte label plus the surrounding format text — never the full
        // 200-byte id.
        assert!(!msg.contains(&"s".repeat(100)));
    }

    /// Defensive boundary: the `reason` field is `ErrorId<128>` (a wider cap
    /// than the 64-byte stream-id labels); an over-long reason truncates at
    /// 128 bytes with a trailing `…`, surfaced via `reason_label`.
    #[test]
    fn invalid_input_display_truncates_overlong_reason() {
        let reason = reason_label(&"r".repeat(400));
        assert!(reason.as_str().len() <= 128);
        assert!(reason.as_str().ends_with('…'));

        let err = FjallError::InvalidInput {
            stream_id: ErrorId::from_display(&"stream-1"),
            version: 3,
            reason,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("stream-1"),
            "display must carry stream id: {msg}"
        );
        assert!(
            msg.contains('…'),
            "display must signal reason truncation: {msg}"
        );
    }
}
