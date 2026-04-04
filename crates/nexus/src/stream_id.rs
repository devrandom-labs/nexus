use std::fmt;

use thiserror::Error;

/// Maximum length (in bytes) for a stream ID.
pub const MAX_STREAM_ID_LEN: usize = 512;

/// A validated, opaque identifier for an event stream.
///
/// Constructed via [`StreamId::new`] (from aggregate name + ID) or
/// [`StreamId::from_persisted`] (from a previously-stored string).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamId(String);

/// Errors returned when constructing a [`StreamId`].
#[derive(Debug, Error)]
pub enum InvalidStreamId {
    /// The raw persisted string was empty.
    #[error("stream ID must not be empty")]
    Empty,

    /// The aggregate name was empty.
    #[error("aggregate name must not be empty")]
    EmptyName,

    /// The aggregate name contains a character outside ASCII alphanumeric and
    /// underscore.
    #[error(
        "aggregate name contains invalid character '{ch}' (only ASCII alphanumeric and underscore allowed)"
    )]
    InvalidNameChar {
        /// The first invalid character found.
        ch: char,
    },

    /// The aggregate ID formatted to an empty string.
    #[error("aggregate ID must not be empty")]
    EmptyId,

    /// The stream ID contains a null byte.
    #[error("stream ID contains null byte")]
    NullByte,

    /// The stream ID exceeds [`MAX_STREAM_ID_LEN`] bytes.
    #[error("stream ID too long: {len} bytes (max {max})")]
    TooLong {
        /// Actual length in bytes.
        len: usize,
        /// Maximum allowed length in bytes.
        max: usize,
    },
}

impl StreamId {
    /// Construct a stream ID from an aggregate name and entity ID.
    ///
    /// # Validation
    ///
    /// - `name` must be non-empty and consist only of ASCII alphanumeric
    ///   characters and underscores.
    /// - `id` must produce a non-empty string when formatted via `Display`.
    /// - The resulting `"{name}-{id}"` must not contain null bytes or exceed
    ///   [`MAX_STREAM_ID_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStreamId`] if any validation check fails.
    pub fn new(name: &str, id: &impl fmt::Display) -> Result<Self, InvalidStreamId> {
        if name.is_empty() {
            return Err(InvalidStreamId::EmptyName);
        }
        for ch in name.chars() {
            if !ch.is_ascii_alphanumeric() && ch != '_' {
                return Err(InvalidStreamId::InvalidNameChar { ch });
            }
        }
        let formatted = format!("{name}-{id}");
        // If the formatted string is exactly name.len() + 1 (the dash), the ID
        // portion was empty.
        if formatted.len() == name.len() + 1 {
            return Err(InvalidStreamId::EmptyId);
        }
        if formatted.contains('\0') {
            return Err(InvalidStreamId::NullByte);
        }
        if formatted.len() > MAX_STREAM_ID_LEN {
            return Err(InvalidStreamId::TooLong {
                len: formatted.len(),
                max: MAX_STREAM_ID_LEN,
            });
        }
        Ok(Self(formatted))
    }

    /// Reconstruct a stream ID from a previously-persisted string.
    ///
    /// This performs only basic sanity checks (non-empty, no null bytes, max
    /// length) and does **not** enforce the `{name}-{id}` format. This allows
    /// store adapters to load stream IDs that may have been written by older
    /// versions of the software.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStreamId`] if the string is empty, contains null bytes,
    /// or exceeds [`MAX_STREAM_ID_LEN`].
    pub fn from_persisted(s: impl Into<String>) -> Result<Self, InvalidStreamId> {
        let inner: String = s.into();
        if inner.is_empty() {
            return Err(InvalidStreamId::Empty);
        }
        if inner.contains('\0') {
            return Err(InvalidStreamId::NullByte);
        }
        if inner.len() > MAX_STREAM_ID_LEN {
            return Err(InvalidStreamId::TooLong {
                len: inner.len(),
                max: MAX_STREAM_ID_LEN,
            });
        }
        Ok(Self(inner))
    }

    /// The stream ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for StreamId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;

    #[test]
    fn new_valid_stream_id() {
        let sid = StreamId::new("Order", &"abc123").unwrap();
        assert_eq!(sid.as_str(), "Order-abc123");
    }

    #[test]
    fn rejects_empty_name() {
        let err = StreamId::new("", &"abc123").unwrap_err();
        assert!(matches!(err, InvalidStreamId::EmptyName));
    }

    #[test]
    fn rejects_empty_id() {
        let err = StreamId::new("Order", &"").unwrap_err();
        assert!(matches!(err, InvalidStreamId::EmptyId));
    }

    #[test]
    fn rejects_invalid_name_chars_dash() {
        let err = StreamId::new("my-aggregate", &"id1").unwrap_err();
        assert!(matches!(err, InvalidStreamId::InvalidNameChar { ch: '-' }));
    }

    #[test]
    fn rejects_invalid_name_chars_slash() {
        let err = StreamId::new("my/aggregate", &"id1").unwrap_err();
        assert!(matches!(err, InvalidStreamId::InvalidNameChar { ch: '/' }));
    }

    #[test]
    fn rejects_invalid_name_chars_unicode() {
        let err = StreamId::new("café", &"id1").unwrap_err();
        assert!(matches!(err, InvalidStreamId::InvalidNameChar { ch: 'é' }));
    }

    #[test]
    fn allows_underscore_in_name() {
        let sid = StreamId::new("my_aggregate", &"id1").unwrap();
        assert_eq!(sid.as_str(), "my_aggregate-id1");
    }

    #[test]
    fn allows_numeric_name() {
        let sid = StreamId::new("Order2", &"id").unwrap();
        assert_eq!(sid.as_str(), "Order2-id");
    }

    #[test]
    fn rejects_null_bytes() {
        let err = StreamId::new("Order", &"abc\0def").unwrap_err();
        assert!(matches!(err, InvalidStreamId::NullByte));
    }

    #[test]
    fn rejects_too_long() {
        let long_id = "x".repeat(MAX_STREAM_ID_LEN);
        let err = StreamId::new("Order", &long_id).unwrap_err();
        assert!(matches!(err, InvalidStreamId::TooLong { .. }));
    }

    #[test]
    fn from_persisted_valid() {
        let sid = StreamId::from_persisted("Order-abc123").unwrap();
        assert_eq!(sid.as_str(), "Order-abc123");
    }

    #[test]
    fn from_persisted_rejects_empty() {
        let err = StreamId::from_persisted("").unwrap_err();
        assert!(matches!(err, InvalidStreamId::Empty));
    }

    #[test]
    fn from_persisted_rejects_null_bytes() {
        let err = StreamId::from_persisted("Order-abc\0def").unwrap_err();
        assert!(matches!(err, InvalidStreamId::NullByte));
    }

    #[test]
    fn from_persisted_rejects_too_long() {
        let long = "x".repeat(MAX_STREAM_ID_LEN + 1);
        let err = StreamId::from_persisted(long).unwrap_err();
        assert!(matches!(err, InvalidStreamId::TooLong { .. }));
    }

    #[test]
    fn from_persisted_accepts_any_format() {
        // Historical compatibility: no requirement for {name}-{id} format.
        let sid = StreamId::from_persisted("no-dash-format").unwrap();
        assert_eq!(sid.as_str(), "no-dash-format");

        let sid2 = StreamId::from_persisted("justaplainstring").unwrap();
        assert_eq!(sid2.as_str(), "justaplainstring");
    }

    #[test]
    fn display_shows_inner_string() {
        let sid = StreamId::new("Order", &"abc123").unwrap();
        assert_eq!(format!("{sid}"), "Order-abc123");
    }

    #[test]
    fn clone_and_eq() {
        let sid = StreamId::new("Order", &"abc123").unwrap();
        let cloned = sid.clone();
        assert_eq!(sid, cloned);
    }

    #[test]
    fn hash_consistency() {
        let sid1 = StreamId::new("Order", &"abc123").unwrap();
        let sid2 = StreamId::new("Order", &"abc123").unwrap();
        assert_eq!(sid1, sid2);

        let mut hasher1 = DefaultHasher::new();
        sid1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        sid2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn as_ref_str() {
        let sid = StreamId::new("Order", &"abc123").unwrap();
        let s: &str = sid.as_ref();
        assert_eq!(s, "Order-abc123");
    }

    /// Compile-time assertion: `StreamId` is `Send + Sync + 'static`.
    const _: () = {
        const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<StreamId>();
    };
}
