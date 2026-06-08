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

use std::num::NonZeroU32;

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
    #[error("payload length {actual} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { actual: usize },
    #[error("metadata length {actual} exceeds maximum {MAX_METADATA_LEN}")]
    MetadataTooLong { actual: usize },
    #[error("metadata is empty; use Option::None to represent absent metadata")]
    MetadataEmpty,
    #[error("schema_version must be > 0 (got 0)")]
    SchemaVersionZero,
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
    /// The read path ([`crate::envelope::PersistedEnvelope::event_type_owned`])
    /// uses this after `try_new` has already validated.
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
    /// The read path ([`crate::envelope::PersistedEnvelope::payload_owned`])
    /// uses this after the buffer's length has been implicitly capped by
    /// the wire format's `u32` length field.
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

/// Validated envelope metadata.
///
/// Invariants:
/// - Length is in `1..=MAX_METADATA_LEN`. Empty metadata is rejected —
///   use `Option::<Metadata>::None` to represent "absent." This avoids
///   the `bytes::Bytes::slice(empty)` footgun where an empty slice
///   orphans from the parent buffer's `STATIC_VTABLE` (the footgun is
///   documented on [`crate::envelope::PersistedEnvelope::metadata_bytes`]).
/// - The non-empty invariant is wire-format-driven: `u32::MAX` is the
///   absent-metadata sentinel in the wire encoding, so `Some(Metadata)`
///   on the read path always carries actual bytes.
///
/// Backed by [`Bytes`] for Arc-shared ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Metadata {
    inner: Bytes,
}

impl Metadata {
    /// Construct from arbitrary bytes, validating non-empty and size cap.
    ///
    /// # Errors
    ///
    /// - [`ValueError::MetadataEmpty`] if `bytes.is_empty()`.
    /// - [`ValueError::MetadataTooLong`] if `bytes.len() > MAX_METADATA_LEN`.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.is_empty() {
            return Err(ValueError::MetadataEmpty);
        }
        if bytes.len() > MAX_METADATA_LEN {
            return Err(ValueError::MetadataTooLong {
                actual: bytes.len(),
            });
        }
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes — crate-internal fast path.
    ///
    /// # Safety
    ///
    /// The caller MUST guarantee:
    /// - `!bytes.is_empty()` (the wire format's `u32::MAX` sentinel handles
    ///   absent metadata; a `Some(Metadata)` must carry actual bytes).
    /// - `bytes.len() <= MAX_METADATA_LEN`.
    ///
    /// Violating these invariants does not produce undefined behavior in
    /// this type's own methods (no `unsafe` consumers), but it will
    /// surface as `WireError::FrameLengthOverflow` at encode time or
    /// trigger the `bytes::Bytes::slice(empty)` `STATIC_VTABLE` footgun
    /// on the read path. Marked `unsafe` for consistency with the other
    /// `from_validated_bytes` constructors and to make the invariant
    /// obligation visible at call sites.
    ///
    /// The read path ([`crate::envelope::PersistedEnvelope::metadata_owned`])
    /// uses this after the wire decoder has rejected the absent sentinel
    /// into `Option::None`.
    pub(crate) unsafe fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            !bytes.is_empty(),
            "from_validated_bytes invariant: non-empty"
        );
        debug_assert!(
            bytes.len() <= MAX_METADATA_LEN,
            "from_validated_bytes invariant: length ≤ MAX_METADATA_LEN"
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

/// Validated schema version — the wire-format-sized sibling of the kernel's
/// [`nexus::Version`].
///
/// Invariant: nonzero, fits in `u32` (the wire format's schema-version field).
///
/// The kernel's `Version` is `NonZeroU64` because aggregate streams can
/// in principle be unboundedly long; `SchemaVersion` is `NonZeroU32`
/// because the wire format chose that width. Same invariant, different
/// width — explicit total conversion to `Version` is provided for the
/// upcaster path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SchemaVersion(NonZeroU32);

impl SchemaVersion {
    /// The initial schema version (`1`).
    pub const INITIAL: Self = Self(NonZeroU32::MIN);

    /// Construct from a `NonZeroU32`. Infallible.
    #[must_use]
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    /// Construct from a raw `u32`, validating the nonzero invariant.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::SchemaVersionZero`] if `value == 0`.
    pub fn from_u32(value: u32) -> Result<Self, ValueError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(ValueError::SchemaVersionZero)
    }

    /// The inner `u32` value (always > 0).
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl From<SchemaVersion> for nexus::Version {
    /// Widen to the kernel's `Version` (`NonZeroU64`). Total — `NonZeroU32`
    /// always fits in `NonZeroU64`.
    #[allow(
        clippy::expect_used,
        reason = "NonZeroU32 widened to u64 is structurally nonzero; the None arm of Version::new is unreachable"
    )]
    fn from(sv: SchemaVersion) -> Self {
        Self::new(u64::from(sv.get())).expect("NonZeroU32 widened to u64 is nonzero")
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

    #[test]
    #[should_panic(expected = "exceeds MAX_EVENT_TYPE_LEN")]
    fn from_static_str_panics_on_leaked_oversize_string() {
        // Box::leak of a runtime-built String over MAX_EVENT_TYPE_LEN is the
        // pathological case the runtime assert! catches. A future refactor
        // that turns the assert! into debug_assert! (compiled out in release)
        // would regress this guard silently — this test makes that regression
        // a hard error.
        let big = "a".repeat(MAX_EVENT_TYPE_LEN + 1);
        let leaked: &'static str = Box::leak(big.into_boxed_str());
        let _ = EventType::from_static_str(leaked);
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

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn from_bytes_accepts_small() {
        let m = Metadata::from_bytes(Bytes::from_static(b"meta")).expect("valid");
        assert_eq!(m.as_slice(), b"meta");
    }

    #[test]
    fn from_bytes_rejects_empty() {
        let err = Metadata::from_bytes(Bytes::new())
            .expect_err("empty metadata must be rejected — use Option::None for absent");
        assert!(matches!(err, ValueError::MetadataEmpty));
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let m = Metadata::from_bytes(Bytes::from_static(b"m")).expect("valid");
        let bytes = m.into_bytes();
        assert_eq!(bytes.as_ref(), b"m");
    }
}

#[cfg(test)]
mod schema_version_tests {
    use super::*;
    use std::num::NonZeroU32;

    #[test]
    fn new_accepts_nonzero() {
        let nz = NonZeroU32::new(1).expect("nonzero");
        let sv = SchemaVersion::new(nz);
        assert_eq!(sv.get(), 1);
    }

    #[test]
    fn initial_is_one() {
        assert_eq!(SchemaVersion::INITIAL.get(), 1);
    }

    #[test]
    fn from_u32_accepts_nonzero() {
        let sv = SchemaVersion::from_u32(42).expect("nonzero");
        assert_eq!(sv.get(), 42);
    }

    #[test]
    fn from_u32_rejects_zero() {
        let err = SchemaVersion::from_u32(0).expect_err("zero rejected");
        assert!(matches!(err, ValueError::SchemaVersionZero));
    }

    #[test]
    fn into_version_widens_to_nonzero_u64() {
        let sv = SchemaVersion::from_u32(7).expect("nonzero");
        let v: nexus::Version = sv.into();
        assert_eq!(v.as_u64(), 7);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use bytes::Bytes;
    use proptest::prelude::*;
    use std::num::NonZeroU32;

    proptest! {
        #[test]
        fn event_type_from_bytes_roundtrips_valid_utf8(s in "[a-zA-Z][a-zA-Z0-9_]{0,63}") {
            let bytes = Bytes::from(s.clone().into_bytes());
            let et = EventType::from_bytes(bytes).expect("valid");
            prop_assert_eq!(et.as_str(), &s);
        }

        #[test]
        fn payload_arbitrary_bytes_within_cap_accepted(
            buf in proptest::collection::vec(any::<u8>(), 0..4096),
        ) {
            let p = Payload::from_bytes(Bytes::from(buf.clone())).expect("under cap");
            prop_assert_eq!(p.as_slice(), buf.as_slice());
        }

        #[test]
        fn metadata_nonempty_within_cap_accepted(
            buf in proptest::collection::vec(any::<u8>(), 1..4096),
        ) {
            let m = Metadata::from_bytes(Bytes::from(buf.clone())).expect("nonempty, under cap");
            prop_assert_eq!(m.as_slice(), buf.as_slice());
        }

        #[test]
        fn event_type_invalid_utf8_always_rejected(
            bytes in proptest::collection::vec(any::<u8>(), 1..256),
        ) {
            // Filter to bytes containing 0xFE or 0xFF, which are never valid
            // UTF-8 start bytes — guarantees the input is invalid UTF-8.
            prop_assume!(bytes.iter().any(|&b| b == 0xFEu8 || b == 0xFFu8));
            let result = EventType::from_bytes(Bytes::from(bytes));
            prop_assert!(result.is_err());
        }

        #[test]
        fn schema_version_from_u32_roundtrips_nonzero(value in 1u32..) {
            let sv = SchemaVersion::from_u32(value).expect("nonzero");
            prop_assert_eq!(sv.get(), value);
        }

        #[test]
        fn schema_version_widens_to_version(value in 1u32..) {
            let sv = SchemaVersion::from_u32(value).expect("nonzero");
            let v: nexus::Version = sv.into();
            prop_assert_eq!(v.as_u64(), u64::from(value));
        }

        #[test]
        fn schema_version_ord_agrees_with_inner_u32_ord(
            a in 1u32..,
            b in 1u32..,
        ) {
            let sa = SchemaVersion::from_u32(a).expect("nonzero");
            let sb = SchemaVersion::from_u32(b).expect("nonzero");
            prop_assert_eq!(sa.cmp(&sb), a.cmp(&b));
        }
    }

    // Pure unit tests for the structural boundaries that proptest can't
    // exercise cheaply (4 GiB+ allocations).
    #[test]
    fn metadata_empty_always_rejected() {
        let err = Metadata::from_bytes(Bytes::new()).expect_err("empty metadata always rejected");
        assert!(matches!(err, ValueError::MetadataEmpty));
    }

    #[test]
    fn schema_version_zero_rejected() {
        let err = SchemaVersion::from_u32(0).expect_err("zero rejected");
        assert!(matches!(err, ValueError::SchemaVersionZero));
    }

    #[test]
    fn schema_version_initial_equals_one() {
        assert_eq!(SchemaVersion::INITIAL.get(), 1);
    }

    #[test]
    fn schema_version_initial_widens_to_version_one() {
        let v: nexus::Version = SchemaVersion::INITIAL.into();
        assert_eq!(v.as_u64(), 1);
    }

    #[test]
    fn schema_version_new_from_nonzero_construction() {
        let nz = NonZeroU32::new(99).expect("nonzero");
        let sv = SchemaVersion::new(nz);
        assert_eq!(sv.get(), 99);
    }
}
