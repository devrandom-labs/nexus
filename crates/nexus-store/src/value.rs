//! Validated value newtypes for envelope fields.
//!
//! Each newtype owns the wire-format invariants for one envelope field:
//! UTF-8 validity (where applicable) and the size cap dictated by the
//! wire format's length-prefix field. Once a value is constructed, it
//! is by definition wire-encodable; downstream layers (`wire.rs`,
//! adapters) skip re-validation.
//!
//! Backed by `bytes::Bytes` for cheap Arc-shared ownership. The
//! `Bytes::from_static` path makes literal event-type names
//! allocation-free, matching the previous `&'static str` ergonomics.

use bytes::Bytes;
use thiserror::Error;

/// Maximum event-type length (the wire format reserves a `u16` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u16→usize widening; lossless on all targets"
)]
pub const MAX_EVENT_TYPE_LEN: usize = u16::MAX as usize;

/// Maximum metadata length. One less than `u32::MAX` because the wire
/// format uses `u32::MAX` as the absent-metadata sentinel.
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_METADATA_LEN: usize = (u32::MAX - 1) as usize;

/// Maximum payload length (the wire format reserves a `u32` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// Construction errors for value newtypes.
#[derive(Debug, Error)]
pub enum ValueError {
    #[error("event_type length {actual} exceeds maximum {MAX_EVENT_TYPE_LEN}")]
    EventTypeTooLong { actual: usize },
    #[error("invalid UTF-8 in event_type bytes (at byte {valid_up_to})")]
    EventTypeInvalidUtf8 {
        valid_up_to: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("metadata length {actual} exceeds maximum {MAX_METADATA_LEN}")]
    MetadataTooLong { actual: usize },
    #[error("payload length {actual} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { actual: usize },
}

/// A validated event type name.
///
/// Invariants: valid UTF-8, length ≤ [`MAX_EVENT_TYPE_LEN`].
///
/// Backed by [`Bytes`]; constructible from a `&'static str` literal at
/// zero allocation via [`from_static_str`](Self::from_static_str), or
/// from arbitrary [`Bytes`] via the validating
/// [`from_bytes`](Self::from_bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventType {
    inner: Bytes,
}

impl EventType {
    /// Construct from a `&'static str` literal. Infallible for valid
    /// literals: `&str` is UTF-8 by Rust's type system, and program-text
    /// literals are bounded by the source file.
    ///
    /// # Panics
    ///
    /// Panics if `s.len() > MAX_EVENT_TYPE_LEN`. For string literals this
    /// is a programmer error caught at first use; the runtime check
    /// catches the `Box::leak(String::from(runtime))` case that would
    /// otherwise silently corrupt the wire encoding.
    #[must_use]
    pub fn from_static_str(s: &'static str) -> Self {
        assert!(
            s.len() <= MAX_EVENT_TYPE_LEN,
            "event_type length {} exceeds MAX_EVENT_TYPE_LEN ({})",
            s.len(),
            MAX_EVENT_TYPE_LEN,
        );
        Self {
            inner: Bytes::from_static(s.as_bytes()),
        }
    }

    /// Construct from arbitrary bytes, validating UTF-8 and size cap.
    ///
    /// # Errors
    ///
    /// - [`ValueError::EventTypeTooLong`] if `bytes.len() > MAX_EVENT_TYPE_LEN`.
    /// - [`ValueError::EventTypeInvalidUtf8`] if `bytes` is not valid UTF-8.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.len() > MAX_EVENT_TYPE_LEN {
            return Err(ValueError::EventTypeTooLong {
                actual: bytes.len(),
            });
        }
        std::str::from_utf8(&bytes).map_err(|e| ValueError::EventTypeInvalidUtf8 {
            valid_up_to: e.valid_up_to(),
            source: e,
        })?;
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes — crate-internal fast path.
    ///
    /// # Safety
    ///
    /// The caller MUST guarantee both invariants:
    /// - `bytes` is valid UTF-8.
    /// - `bytes.len() <= MAX_EVENT_TYPE_LEN`.
    ///
    /// Violating either invariant makes [`EventType::as_str`] undefined
    /// behavior in release builds. The `debug_assert!`s below are
    /// diagnostic-only and compiled out in release.
    ///
    /// The read path ([`crate::envelope::PersistedEnvelope::event_type_owned`],
    /// added in a later PR) uses this after `try_new` has already validated.
    #[expect(
        dead_code,
        reason = "wired up by the PersistedEnvelope read-path task later in this PR series"
    )]
    pub(crate) unsafe fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            bytes.len() <= MAX_EVENT_TYPE_LEN,
            "from_validated_bytes invariant: length ≤ MAX_EVENT_TYPE_LEN"
        );
        debug_assert!(
            std::str::from_utf8(&bytes).is_ok(),
            "from_validated_bytes invariant: valid UTF-8"
        );
        Self { inner: bytes }
    }

    /// Borrow as `&str`. Zero-cost; UTF-8 is guaranteed by construction.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: every constructor validates UTF-8 (or accepts
        // `&'static str` which is UTF-8 by Rust's type system).
        #[allow(
            unsafe_code,
            reason = "UTF-8 invariant established by every constructor"
        )]
        unsafe {
            std::str::from_utf8_unchecked(&self.inner)
        }
    }

    /// Borrow as `&[u8]`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner
    }

    /// Take ownership of the inner [`Bytes`] (one Arc share, no copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.inner
    }
}

/// A validated event payload.
///
/// Invariant: length ≤ [`MAX_PAYLOAD_LEN`].
///
/// Backed by [`Bytes`] for Arc-shared ownership. Empty payloads are
/// accepted — unlike [`Metadata`], an empty payload is a legal event
/// shape (e.g., a marker event with no data).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Payload {
    inner: Bytes,
}

impl Payload {
    /// Construct from arbitrary bytes, validating the size cap.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::PayloadTooLong`] if `bytes.len() > MAX_PAYLOAD_LEN`.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.len() > MAX_PAYLOAD_LEN {
            return Err(ValueError::PayloadTooLong {
                actual: bytes.len(),
            });
        }
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes — crate-internal fast path.
    ///
    /// # Safety
    ///
    /// The caller MUST guarantee `bytes.len() <= MAX_PAYLOAD_LEN`.
    ///
    /// Violating the invariant does not produce undefined behavior in this
    /// type's own methods (no `unsafe` consumers), but it will corrupt the
    /// wire encoding downstream when [`crate::wire::encode_frame`] tries
    /// to fit the length into a `u32` field. Marked `unsafe` for
    /// consistency with the other `from_validated_bytes` constructors in
    /// this module and to make the invariant obligation visible at call
    /// sites.
    ///
    /// The read path ([`crate::envelope::PersistedEnvelope::payload_owned`],
    /// added in a later PR) uses this after the buffer's length has been
    /// implicitly capped by the wire format's `u32` length field.
    #[expect(
        dead_code,
        reason = "wired up by the PersistedEnvelope read-path task later in this PR series"
    )]
    pub(crate) unsafe fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            bytes.len() <= MAX_PAYLOAD_LEN,
            "from_validated_bytes invariant: length ≤ MAX_PAYLOAD_LEN"
        );
        Self { inner: bytes }
    }

    /// Borrow as `&[u8]`. Zero-cost.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    /// Take ownership of the inner [`Bytes`] (one Arc share, no copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.inner
    }
}

#[cfg(test)]
mod event_type_tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn from_static_str_accepts_literal() {
        let et = EventType::from_static_str("UserCreated");
        assert_eq!(et.as_str(), "UserCreated");
    }

    #[test]
    fn from_bytes_accepts_valid_utf8_within_cap() {
        let et = EventType::from_bytes(Bytes::from_static(b"OrderPlaced"))
            .expect("valid utf-8 within cap");
        assert_eq!(et.as_str(), "OrderPlaced");
    }

    #[test]
    fn from_bytes_rejects_invalid_utf8() {
        let err = EventType::from_bytes(Bytes::from_static(&[0xFFu8, 0xFE]))
            .expect_err("must reject invalid utf-8");
        assert!(matches!(err, ValueError::EventTypeInvalidUtf8 { .. }));
    }

    #[test]
    fn from_bytes_rejects_oversize() {
        let too_big = Bytes::from(vec![b'a'; MAX_EVENT_TYPE_LEN + 1]);
        let err =
            EventType::from_bytes(too_big).expect_err("must reject length > MAX_EVENT_TYPE_LEN");
        assert!(
            matches!(err, ValueError::EventTypeTooLong { actual } if actual == MAX_EVENT_TYPE_LEN + 1)
        );
    }

    #[test]
    fn at_max_cap_accepted() {
        let at_cap = Bytes::from(vec![b'a'; MAX_EVENT_TYPE_LEN]);
        let et = EventType::from_bytes(at_cap).expect("length at cap is valid");
        assert_eq!(et.as_bytes().len(), MAX_EVENT_TYPE_LEN);
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let et = EventType::from_static_str("Foo");
        let bytes = et.into_bytes();
        assert_eq!(bytes.as_ref(), b"Foo");
    }

    #[test]
    fn debug_redacts_nothing_short_names_show_in_full() {
        let et = EventType::from_static_str("Foo");
        let dbg = format!("{et:?}");
        assert!(dbg.contains("Foo"), "Debug should include event type name");
        assert!(
            dbg.len() < 40,
            "Debug should be tight (no padding/extra fields): got {} chars: {dbg}",
            dbg.len(),
        );
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn from_bytes_accepts_empty() {
        let p = Payload::from_bytes(Bytes::new()).expect("empty payload is valid");
        assert!(p.as_slice().is_empty());
    }

    #[test]
    fn from_bytes_accepts_small() {
        let p = Payload::from_bytes(Bytes::from_static(b"hello")).expect("valid");
        assert_eq!(p.as_slice(), b"hello");
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let p = Payload::from_bytes(Bytes::from_static(b"payload")).expect("valid");
        let bytes = p.into_bytes();
        assert_eq!(bytes.as_ref(), b"payload");
    }

    // Note: oversize check (length > MAX_PAYLOAD_LEN = u32::MAX) is
    // covered by the property tests in Task 1.6 with a synthetic length
    // boundary. Allocating ~4 GB in a unit test is impractical.
}
