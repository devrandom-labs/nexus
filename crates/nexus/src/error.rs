use crate::version::Version;
use thiserror::Error;

/// Fixed-size stack-allocated string for error messages.
/// No heap allocation — safe to construct on error paths under OOM.
/// Truncates at 64 bytes if the formatted value is longer.
#[derive(Clone, PartialEq, Eq)]
pub struct ErrorId {
    buf: [u8; 64],
    len: u8,
}

impl ErrorId {
    /// Create from any `Display` type — no heap allocation.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "to_copy is capped at 64 which fits in u8"
    )]
    pub fn from_display(value: &impl std::fmt::Display) -> Self {
        use std::fmt::Write;
        let mut id = Self {
            buf: [0; 64],
            len: 0,
        };
        let _ = write!(id, "{value}");
        id
    }
}

impl std::fmt::Write for ErrorId {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "len and to_copy are bounded to 64, which fits in u8 and usize"
    )]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let remaining = 64 - usize::from(self.len);
        let to_copy = s.len().min(remaining);
        let start = usize::from(self.len);
        self.buf[start..start + to_copy].copy_from_slice(&s.as_bytes()[..to_copy]);
        self.len += to_copy as u8;
        Ok(())
    }
}

impl std::fmt::Display for ErrorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = std::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("<invalid utf8>");
        f.write_str(s)
    }
}

impl std::fmt::Debug for ErrorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{self}\"")
    }
}

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("Version mismatch on '{stream_id}': expected {expected}, got {actual}")]
    VersionMismatch {
        stream_id: ErrorId,
        expected: Version,
        actual: Version,
    },

    #[error("Rehydration limit exceeded on '{stream_id}': max {max} events")]
    RehydrationLimitExceeded { stream_id: ErrorId, max: usize },
}
