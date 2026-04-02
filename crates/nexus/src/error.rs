use crate::version::Version;
use thiserror::Error;

/// Fixed-size stack-allocated string for error messages.
/// No heap allocation — safe to construct on error paths under OOM.
/// Truncates at 128 bytes if the formatted value is longer.
#[derive(Clone, PartialEq, Eq)]
pub struct ErrorId {
    buf: [u8; 128],
    len: u8,
}

impl ErrorId {
    /// Create from any `Display` type — no heap allocation.
    pub fn from_display(value: &impl std::fmt::Display) -> Self {
        use std::fmt::Write;
        let mut id = Self {
            buf: [0; 128],
            len: 0,
        };
        // Write into the fixed buffer, truncating if too long
        let _ = write!(id, "{value}");
        id
    }
}

impl std::fmt::Write for ErrorId {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let remaining = 128 - self.len as usize;
        let to_copy = s.len().min(remaining);
        self.buf[self.len as usize..self.len as usize + to_copy]
            .copy_from_slice(&s.as_bytes()[..to_copy]);
        self.len += to_copy as u8;
        Ok(())
    }
}

impl std::fmt::Display for ErrorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("<invalid utf8>");
        f.write_str(s)
    }
}

impl std::fmt::Debug for ErrorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{}\"", self)
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
