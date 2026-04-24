use std::fmt::{Debug, Display, Write};
use std::hash::Hash;

use arrayvec::ArrayString;

/// Find the largest byte position at or before `limit` that sits on a
/// UTF-8 char boundary in `s`. Returns 0 if no valid boundary exists.
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

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {
    /// Fixed byte length of this ID's storage representation.
    /// All instances must return exactly this many bytes from `as_ref()`.
    const BYTE_LEN: usize;

    /// Stack-allocated diagnostic label for error messages.
    /// Truncates at a char boundary if the display representation exceeds 64 bytes.
    fn to_label(&self) -> ArrayString<64> {
        /// Wrapper that fills an `ArrayString` with partial writes,
        /// silently dropping overflow instead of returning `Err`.
        struct LabelWriter(ArrayString<64>);

        impl Write for LabelWriter {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                let remaining = 64 - self.0.len();
                if remaining == 0 {
                    return Ok(());
                }
                let end = truncate_at_char_boundary(s, remaining);
                // Safety: end is at a char boundary and end <= remaining <= capacity - len.
                self.0.try_push_str(&s[..end]).ok();
                Ok(())
            }
        }

        let mut writer = LabelWriter(ArrayString::new());
        let _ = write!(writer, "{self}");
        writer.0
    }
}
