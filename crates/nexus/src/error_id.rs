//! [`ErrorId`] — a bounded, stack-allocated diagnostic label that keeps
//! `arrayvec::ArrayString` out of the public API.
//!
//! Error types across the kernel, store, and fjall adapter carry short
//! identifier / reason strings (stream ids, failure reasons) sized for
//! `IoT` / embedded targets — no heap allocation on error paths. Exposing
//! `ArrayString<N>` in those public fields would couple our `SemVer` to
//! `arrayvec` 0.x: a major bump there would be a breaking change here.
//! `ErrorId<N>` carries the same bytes without leaking the dependency.
//!
//! Construction is truncation-aware: [`ErrorId::from_display`] fills the
//! buffer and, if the source did not fit, replaces the tail with `…`
//! (U+2026) on a char boundary — visually signalling truncation. Silent
//! truncation (the old `to_label` / `reason_label` behaviour) is not
//! offered, so there is no `From<&str>` back door.

use core::fmt::{self, Debug, Display, Write};

use arrayvec::ArrayString;

/// Default capacity for an [`ErrorId`]: 64 bytes, matching stream-id labels.
pub const DEFAULT_ERROR_ID_CAP: usize = 64;

/// Truncation marker appended when a source value overflows the buffer.
/// `…` (U+2026) is 3 bytes in UTF-8.
const ELLIPSIS: &str = "…";

/// Largest byte position at or before `limit` that sits on a UTF-8 char
/// boundary in `s`. Returns `s.len()` when `limit >= s.len()`, and 0 when
/// no in-range boundary exists.
const fn truncate_at_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut pos = limit;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// A bounded, stack-allocated diagnostic label of at most `N` UTF-8 bytes.
///
/// Construct via [`ErrorId::from_display`]; longer values are truncated on a
/// char boundary and suffixed with `…`. `N` defaults to
/// [`DEFAULT_ERROR_ID_CAP`] (64).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorId<const N: usize = DEFAULT_ERROR_ID_CAP> {
    inner: ArrayString<N>,
}

impl<const N: usize> ErrorId<N> {
    /// An empty label.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: ArrayString::new(),
        }
    }

    /// Build a label from a [`Display`] value, truncating on a char
    /// boundary with a trailing `…` if it exceeds `N` bytes.
    #[must_use]
    pub fn from_display(value: &impl Display) -> Self {
        let mut filler = Filler::<N>::new();
        // `Filler::write_str` always returns `Ok`, so `write!` cannot fail.
        let _ = write!(filler, "{value}");
        filler.finish()
    }

    /// The label as a string slice. Always valid UTF-8, length `<= N`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    /// Number of bytes in the label.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the label is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<const N: usize> Default for ErrorId<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Display for ErrorId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> Debug for ErrorId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print like a string literal so error `{:?}` output reads cleanly.
        write!(f, "{:?}", self.as_str())
    }
}

/// Fills an `ArrayString<N>` from `fmt::Write` chunks, recording whether
/// any input was dropped so the tail can be marked with `…`.
struct Filler<const N: usize> {
    buf: ArrayString<N>,
    truncated: bool,
}

impl<const N: usize> Filler<N> {
    fn new() -> Self {
        Self {
            buf: ArrayString::new(),
            truncated: false,
        }
    }

    fn finish(self) -> ErrorId<N> {
        if self.truncated {
            ErrorId {
                inner: append_ellipsis(self.buf),
            }
        } else {
            ErrorId { inner: self.buf }
        }
    }
}

impl<const N: usize> Write for Filler<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.truncated {
            // Already overflowed; nothing more fits.
            return Ok(());
        }
        let remaining = N - self.buf.len();
        if s.len() <= remaining {
            self.buf.try_push_str(s).ok();
            return Ok(());
        }
        let end = truncate_at_char_boundary(s, remaining);
        self.buf.try_push_str(&s[..end]).ok();
        self.truncated = true;
        Ok(())
    }
}

/// Replace the tail of `buf` with `…` on a char boundary so the result is
/// `<= N` bytes. If `N` cannot hold the 3-byte marker (never the case for
/// the 64/128 widths in use), the buffer is returned without a marker.
fn append_ellipsis<const N: usize>(mut buf: ArrayString<N>) -> ArrayString<N> {
    if N < ELLIPSIS.len() {
        return buf;
    }
    let target = N - ELLIPSIS.len();
    let cut = truncate_at_char_boundary(buf.as_str(), target);
    buf.truncate(cut);
    // `cut <= N - 3` and the marker is 3 bytes, so this always fits.
    buf.try_push_str(ELLIPSIS).ok();
    buf
}
