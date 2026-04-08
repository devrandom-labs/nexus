/// Diagnostic label identifying a stream in error messages.
///
/// Constructed automatically from any [`Id`](nexus::Id) via the
/// [`ToStreamLabel`] trait (blanket-implemented for all `Display` types).
///
/// # Representation
///
/// - **Default (std):** heap-allocated `String` via `to_string()`.
/// - **`bounded-labels` feature (IoT/embedded):** 64-byte stack buffer,
///   truncated with `...` suffix. No heap allocation — safe on error
///   paths under OOM.
#[cfg(not(feature = "bounded-labels"))]
#[derive(Clone, PartialEq, Eq)]
pub struct StreamLabel(String);

#[cfg(not(feature = "bounded-labels"))]
impl StreamLabel {
    pub(crate) fn from_display(value: &(impl std::fmt::Display + ?Sized)) -> Self {
        Self(value.to_string())
    }
}

#[cfg(not(feature = "bounded-labels"))]
impl std::fmt::Display for StreamLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(not(feature = "bounded-labels"))]
impl std::fmt::Debug for StreamLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{self}\"")
    }
}

// =============================================================================
// bounded-labels variant — 64-byte stack buffer for IoT/embedded
// =============================================================================

/// Maximum byte length of the bounded label buffer.
#[cfg(feature = "bounded-labels")]
const MAX_LEN: usize = 64;

/// Truncation suffix appended when content exceeds the buffer.
#[cfg(feature = "bounded-labels")]
const ELLIPSIS: &[u8; 3] = b"...";

#[cfg(feature = "bounded-labels")]
#[derive(Clone, PartialEq, Eq)]
pub struct StreamLabel {
    buf: [u8; MAX_LEN],
    /// Number of valid bytes in `buf`. Always <= MAX_LEN.
    len: u8,
    /// Set to `true` by `write_str` when content was dropped due to
    /// the buffer being full. Used by `from_display` to decide whether
    /// to append the `...` truncation suffix.
    truncated: bool,
}

#[cfg(feature = "bounded-labels")]
impl StreamLabel {
    /// Find the largest char-aligned byte position at or before `limit`.
    ///
    /// Walks backwards from `limit` to avoid splitting multi-byte
    /// UTF-8 codepoints. Returns 0 if no valid boundary exists.
    fn char_boundary(buf: &[u8], limit: usize) -> usize {
        let mut pos = limit;
        // Position `buf.len()` is always a valid boundary (end of buffer).
        // For positions inside the buffer, walk back past continuation bytes.
        while pos > 0 && pos < buf.len() && !is_utf8_leading_byte(buf[pos]) {
            pos -= 1;
        }
        pos
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "len is bounded to MAX_LEN (64) which fits in u8"
    )]
    pub(crate) fn from_display(value: &(impl std::fmt::Display + ?Sized)) -> Self {
        use std::fmt::Write;
        let mut label = Self {
            buf: [0; MAX_LEN],
            len: 0,
            truncated: false,
        };
        let _ = write!(label, "{value}");

        if label.truncated {
            // Content exceeded MAX_LEN. Find a safe char boundary for
            // the prefix, leaving room for "...".
            let safe = Self::char_boundary(&label.buf, MAX_LEN - ELLIPSIS.len());
            label.buf[safe..safe + ELLIPSIS.len()].copy_from_slice(ELLIPSIS);
            label.len = (safe + ELLIPSIS.len()) as u8;
        } else {
            // No overflow, but write_str may have split a multi-byte
            // codepoint at the buffer boundary. Trim to a valid boundary.
            let len = usize::from(label.len);
            let safe = Self::char_boundary(&label.buf, len);
            label.len = safe as u8;
        }

        label
    }
}

/// Returns `true` if `byte` is a UTF-8 leading byte (not a continuation byte).
///
/// UTF-8 continuation bytes have the bit pattern `10xxxxxx`.
/// Leading bytes are either ASCII (`0xxxxxxx`) or multi-byte starts (`11xxxxxx`).
#[cfg(feature = "bounded-labels")]
const fn is_utf8_leading_byte(byte: u8) -> bool {
    // Continuation bytes: 0b10xx_xxxx (0x80..=0xBF)
    byte & 0b1100_0000 != 0b1000_0000
}

#[cfg(feature = "bounded-labels")]
impl std::fmt::Write for StreamLabel {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "len and to_copy are bounded to MAX_LEN (64), which fits in u8"
    )]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let remaining = MAX_LEN - usize::from(self.len);
        let to_copy = s.len().min(remaining);
        let start = usize::from(self.len);
        self.buf[start..start + to_copy].copy_from_slice(&s.as_bytes()[..to_copy]);
        self.len += to_copy as u8;
        if s.len() > remaining {
            self.truncated = true;
        }
        Ok(())
    }
}

#[cfg(feature = "bounded-labels")]
impl std::fmt::Display for StreamLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // from_display guarantees buf[0..len] is char-aligned valid UTF-8.
        let s = std::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("<invalid utf8>");
        f.write_str(s)
    }
}

#[cfg(feature = "bounded-labels")]
impl std::fmt::Debug for StreamLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{self}\"")
    }
}

// =============================================================================
// ToStreamLabel trait — blanket impl for all Display types
// =============================================================================

/// Convert any `Display` type into a [`StreamLabel`] for diagnostics.
///
/// Blanket-implemented for all `Display` types, so any [`Id`](nexus::Id)
/// works automatically:
///
/// ```ignore
/// use nexus_store::ToStreamLabel;
/// let label: StreamLabel = my_aggregate_id.to_stream_label();
/// ```
pub trait ToStreamLabel: std::fmt::Display {
    /// Create a diagnostic stream label from this value.
    fn to_stream_label(&self) -> StreamLabel {
        StreamLabel::from_display(self)
    }
}

impl<T: std::fmt::Display + ?Sized> ToStreamLabel for T {}
