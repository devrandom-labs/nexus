use std::ops::Range;

use bytes::Bytes;
use nexus::Version;
use thiserror::Error;

use crate::store::GlobalSeq;
use crate::value::{
    EventType, MAX_EVENT_TYPE_LEN, MAX_METADATA_LEN, Metadata, Payload, SchemaVersion, ValueError,
};

/// Cast `u32` index to `usize` for slice indexing.
///
/// `u32 as usize` is lossless on all platforms Nexus supports (32-bit+).
/// 16-bit platforms are not supported; this is a deliberate architecture constraint.
#[allow(
    clippy::as_conversions,
    reason = "u32→usize is lossless on all Nexus target platforms (32-bit+)"
)]
#[inline]
const fn idx(n: u32) -> usize {
    n as usize
}

// =============================================================================
// Errors
// =============================================================================

/// Errors from envelope construction.
#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("range {start}..{end} exceeds buffer length {len}")]
    RangeOutOfBounds { start: u32, end: u32, len: usize },

    #[error("invalid UTF-8 in event_type at bytes {start}..{end}")]
    InvalidUtf8 {
        start: u32,
        end: u32,
        #[source]
        source: std::str::Utf8Error,
    },

    /// The `event_type` range length exceeds the wire-format cap.
    ///
    /// Structurally rules out the only path that could otherwise hand
    /// `EventType::from_validated_bytes` an oversize slice on the read
    /// side, so the fast-path accessor is sound by construction.
    #[error("event_type range length {actual} exceeds maximum {max}")]
    EventTypeRangeTooLong { actual: u32, max: usize },

    /// The metadata range length exceeds the wire-format cap.
    ///
    /// Mirrors `EventTypeRangeTooLong` for metadata.
    #[error("metadata range length {actual} exceeds maximum {max}")]
    MetadataRangeTooLong { actual: u32, max: usize },

    /// `Some(range)` was passed where `range` is empty. The wire format
    /// reserves the absent sentinel (`meta_len == u32::MAX`) for "no
    /// metadata"; an empty range collides with the `Bytes::slice(empty)`
    /// `STATIC_VTABLE` orphan footgun and the value newtype invariant
    /// `!Metadata::is_empty()`.
    #[error("metadata range is empty; use None to represent absent metadata")]
    MetadataRangeEmpty,

    #[error(transparent)]
    Value(#[from] ValueError),
}

/// Errors from [`PersistedEnvelope::for_decode`].
///
/// Combines the failure modes of the underlying value-newtype
/// construction, the wire encode, and the envelope `try_new` — `?`
/// promotes any of the three.
#[derive(Debug, Error)]
pub enum ForDecodeError {
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error(transparent)]
    Wire(#[from] crate::wire::WireError),
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
}

// =============================================================================
// PendingEnvelope — write path, fully owned (Bytes), no lifetime, no generic
// =============================================================================

/// Event envelope for the write path.
///
/// Fields hold validated value newtypes — `EventType`, `Payload`, `Metadata`,
/// `SchemaVersion` — so downstream wire encoding can skip re-validation.
///
/// Construction is via the typestate builder rooted at [`pending_envelope`].
#[derive(Debug, Clone)]
pub struct PendingEnvelope {
    version: Version,
    event_type: EventType,
    schema_version: SchemaVersion,
    payload: Payload,
    metadata: Option<Metadata>,
}

impl PendingEnvelope {
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// Borrowed event type as `&str`.
    #[must_use]
    pub fn event_type(&self) -> &str {
        self.event_type.as_str()
    }

    /// Owned event type — one Arc share.
    #[must_use]
    pub fn event_type_value(&self) -> EventType {
        self.event_type.clone()
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.payload.as_slice()
    }

    /// Owned payload — one Arc share.
    #[must_use]
    pub fn payload_bytes(&self) -> Bytes {
        self.payload.clone().into_bytes()
    }

    /// Owned payload value newtype — one Arc share.
    #[must_use]
    pub fn payload_value(&self) -> Payload {
        self.payload.clone()
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_ref().map(Metadata::as_slice)
    }

    /// Owned metadata — one Arc share per `Some`.
    #[must_use]
    pub fn metadata_bytes(&self) -> Option<Bytes> {
        self.metadata.as_ref().map(|m| m.clone().into_bytes())
    }

    /// Owned metadata value newtype — one Arc share per `Some`.
    #[must_use]
    pub fn metadata_value(&self) -> Option<Metadata> {
        self.metadata.clone()
    }

    /// The raw u32 view (always > 0, by the `SchemaVersion` invariant).
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version.get()
    }

    /// The typed schema version.
    #[must_use]
    pub const fn schema_version_value(&self) -> SchemaVersion {
        self.schema_version
    }

    /// Rebuild a write-path envelope from a read-path one.
    ///
    /// Reuses the [`PersistedEnvelope`]'s already-validated value newtypes
    /// (one Arc share each — zero copy, no re-validation) and **drops
    /// `global_seq`**: the store stamps a fresh one on append. Infallible,
    /// because every field of a `PersistedEnvelope` was validated at its own
    /// construction. Used by the importer to re-append exported events.
    #[cfg(feature = "import")]
    #[must_use]
    pub(crate) fn from_persisted(persisted: &PersistedEnvelope) -> Self {
        Self {
            version: persisted.version(),
            event_type: persisted.event_type_value(),
            schema_version: persisted.schema_version_value(),
            payload: persisted.payload_value(),
            metadata: persisted.metadata_value(),
        }
    }
}

// =============================================================================
// Typestate builder — compile-time enforced construction
// =============================================================================

/// Step 1: has `version`, needs `event_type`.
#[derive(Debug)]
pub struct WithVersion {
    version: Version,
}

/// Step 2: has `version` + `event_type`, needs payload.
#[derive(Debug)]
pub struct WithEventType {
    version: Version,
    event_type: EventType,
}

/// Step 3: has all core fields; optional `schema_version` override; finalize via `build`/`with_metadata`.
#[derive(Debug)]
pub struct WithPayload {
    version: Version,
    event_type: EventType,
    payload: Payload,
    schema_version: SchemaVersion,
}

impl WithVersion {
    /// Set the event type from a `&'static str` literal. Infallible.
    #[must_use]
    pub fn event_type(self, event_type: &'static str) -> WithEventType {
        WithEventType {
            version: self.version,
            event_type: EventType::from_static_str(event_type),
        }
    }

    /// Set the event type from arbitrary bytes; validates UTF-8 and size cap.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the bytes are invalid UTF-8 or
    /// exceed [`MAX_EVENT_TYPE_LEN`](crate::value::MAX_EVENT_TYPE_LEN).
    pub fn event_type_bytes(self, bytes: Bytes) -> Result<WithEventType, EnvelopeError> {
        let event_type = EventType::from_bytes(bytes)?;
        Ok(WithEventType {
            version: self.version,
            event_type,
        })
    }
}

impl WithEventType {
    /// Set the payload from any `Into<Bytes>` source. Fallible: validates
    /// the size cap.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the payload exceeds
    /// [`MAX_PAYLOAD_LEN`](crate::value::MAX_PAYLOAD_LEN).
    pub fn payload(self, payload: impl Into<Bytes>) -> Result<WithPayload, EnvelopeError> {
        let validated = Payload::from_bytes(payload.into())?;
        Ok(WithPayload {
            version: self.version,
            event_type: self.event_type,
            payload: validated,
            schema_version: SchemaVersion::INITIAL,
        })
    }
}

impl WithPayload {
    /// Override the schema version (default: [`SchemaVersion::INITIAL`]).
    #[must_use]
    pub const fn schema_version(mut self, schema_version: SchemaVersion) -> Self {
        self.schema_version = schema_version;
        self
    }

    /// Build with no metadata. Infallible.
    #[must_use]
    pub fn build(self) -> PendingEnvelope {
        PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata: None,
        }
    }

    /// Build with metadata; fallible (validates non-empty + size cap).
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the metadata is empty or
    /// exceeds [`MAX_METADATA_LEN`](crate::value::MAX_METADATA_LEN).
    pub fn with_metadata(
        self,
        metadata: impl Into<Bytes>,
    ) -> Result<PendingEnvelope, EnvelopeError> {
        let validated = Metadata::from_bytes(metadata.into())?;
        Ok(PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata: Some(validated),
        })
    }
}

/// Start building a `PendingEnvelope`.
///
/// ```ignore
/// pending_envelope(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .build()
/// // or
///     .with_metadata(meta_bytes)
/// ```
#[must_use]
pub const fn pending_envelope(version: Version) -> WithVersion {
    WithVersion { version }
}

// =============================================================================
// PersistedEnvelope — read path, owned via Bytes + Range<u32> offsets
// =============================================================================

/// Event envelope for the read path.
///
/// Holds the whole row value as a single `Bytes` plus `Range<u32>` offsets
/// for `event_type`, `payload`, and (optional) `metadata`. All views share
/// the one Arc; accessors return `&[u8]`/`&str` cheaply or `Bytes` via
/// `value.slice(range)` for owned views.
///
/// Construction validates ranges against the buffer, UTF-8 of `event_type`,
/// and the structural per-field caps that the value newtypes own (so the
/// fast-path `*_value()` accessors are sound without re-validation).
#[derive(Debug, Clone)]
pub struct PersistedEnvelope {
    version: Version,
    global_seq: GlobalSeq,
    schema_version: SchemaVersion,
    value: Bytes,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
}

impl PersistedEnvelope {
    /// Construct from decoded row data, validating ranges and UTF-8.
    ///
    /// # Errors
    ///
    /// - [`EnvelopeError::RangeOutOfBounds`] if any range's `end` exceeds `value.len()`.
    /// - [`EnvelopeError::InvalidUtf8`] if `event_type` bytes are not valid UTF-8.
    /// - [`EnvelopeError::EventTypeRangeTooLong`] if the event-type range
    ///   length exceeds [`MAX_EVENT_TYPE_LEN`].
    /// - [`EnvelopeError::MetadataRangeTooLong`] if the metadata range
    ///   length exceeds [`MAX_METADATA_LEN`].
    /// - [`EnvelopeError::MetadataRangeEmpty`] if `Some(range)` is passed
    ///   with an empty range.
    #[allow(
        clippy::too_many_arguments,
        reason = "all 7 fields are required to construct a validated PersistedEnvelope; \
                  a builder would add indirection with no type-safety benefit here"
    )]
    pub fn try_new(
        version: Version,
        global_seq: GlobalSeq,
        value: Bytes,
        schema_version: SchemaVersion,
        event_type_range: Range<u32>,
        payload_range: Range<u32>,
        metadata_range: Option<Range<u32>>,
    ) -> Result<Self, EnvelopeError> {
        let len = value.len();
        check_range(&event_type_range, len)?;
        check_range(&payload_range, len)?;
        if let Some(ref m) = metadata_range {
            check_range(m, len)?;
        }

        // Structural per-field caps — owned upstream by the value
        // newtypes; mirroring them here makes `event_type_value` /
        // `metadata_value` sound without re-validation.
        let et_range_len = event_type_range.end - event_type_range.start;
        if idx(et_range_len) > MAX_EVENT_TYPE_LEN {
            return Err(EnvelopeError::EventTypeRangeTooLong {
                actual: et_range_len,
                max: MAX_EVENT_TYPE_LEN,
            });
        }
        if let Some(ref m) = metadata_range {
            let meta_range_len = m.end - m.start;
            if meta_range_len == 0 {
                return Err(EnvelopeError::MetadataRangeEmpty);
            }
            if idx(meta_range_len) > MAX_METADATA_LEN {
                return Err(EnvelopeError::MetadataRangeTooLong {
                    actual: meta_range_len,
                    max: MAX_METADATA_LEN,
                });
            }
        }

        // UTF-8 validation of event_type once at construction.
        let et_start = idx(event_type_range.start);
        let et_end = idx(event_type_range.end);
        std::str::from_utf8(&value[et_start..et_end]).map_err(|e| EnvelopeError::InvalidUtf8 {
            start: event_type_range.start,
            end: event_type_range.end,
            source: e,
        })?;
        Ok(Self {
            version,
            global_seq,
            schema_version,
            value,
            event_type_range,
            payload_range,
            metadata_range,
        })
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn global_seq(&self) -> GlobalSeq {
        self.global_seq
    }

    /// The raw `u32` view of `schema_version` (always > 0 by the
    /// [`SchemaVersion`] invariant).
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version.get()
    }

    /// The typed [`SchemaVersion`].
    #[must_use]
    pub const fn schema_version_value(&self) -> SchemaVersion {
        self.schema_version
    }

    /// UTF-8 validated at construction; cheap accessor.
    #[must_use]
    pub fn event_type(&self) -> &str {
        let start = idx(self.event_type_range.start);
        let end = idx(self.event_type_range.end);
        // SAFETY: UTF-8 validated once in `try_new`; range bounds checked
        // there too. The backing `Bytes` is immutable post-construction
        // (no setter, fields private).
        #[allow(
            unsafe_code,
            reason = "UTF-8 invariant established at construction; ranges validated"
        )]
        unsafe {
            std::str::from_utf8_unchecked(&self.value[start..end])
        }
    }

    /// Owned `Bytes` view of `event_type` — one atomic refcount inc.
    #[must_use]
    pub fn event_type_bytes(&self) -> Bytes {
        self.slice_range(&self.event_type_range)
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        let start = idx(self.payload_range.start);
        let end = idx(self.payload_range.end);
        &self.value[start..end]
    }

    /// Owned `Bytes` view of `payload` — one atomic refcount inc.
    #[must_use]
    pub fn payload_bytes(&self) -> Bytes {
        self.slice_range(&self.payload_range)
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata_range.as_ref().map(|r| {
            let start = idx(r.start);
            let end = idx(r.end);
            &self.value[start..end]
        })
    }

    /// Owned `Bytes` view of `metadata` — one atomic refcount inc per `Some`.
    ///
    /// `try_new` rejects `Some(empty)`, so the `Bytes::slice(empty)`
    /// `STATIC_VTABLE` orphan footgun is structurally unreachable here.
    #[must_use]
    pub fn metadata_bytes(&self) -> Option<Bytes> {
        self.metadata_range.as_ref().map(|r| self.slice_range(r))
    }

    /// Validated event type — one Arc share over the underlying buffer.
    #[must_use]
    pub fn event_type_value(&self) -> EventType {
        // SAFETY: `from_validated_bytes` requires (1) valid UTF-8 and
        // (2) `bytes.len() <= MAX_EVENT_TYPE_LEN`. Both invariants are
        // established by `try_new`: UTF-8 via `std::str::from_utf8`, and
        // the cap via the `EventTypeRangeTooLong` check on the range
        // length.
        #[allow(
            unsafe_code,
            reason = "UTF-8 and length cap both established by try_new"
        )]
        unsafe {
            EventType::from_validated_bytes(self.event_type_bytes())
        }
    }

    /// Validated payload — one Arc share over the underlying buffer.
    #[must_use]
    pub fn payload_value(&self) -> Payload {
        // SAFETY: `PersistedEnvelope::try_new` validated `payload_range`
        // against `value.len()` via `check_range`. The range is a
        // `Range<u32>`, so the resulting slice length is bounded by
        // `u32::MAX = MAX_PAYLOAD_LEN`. `payload_bytes()` returns a slice
        // of the same validated buffer.
        #[allow(
            unsafe_code,
            reason = "size invariant established at PersistedEnvelope::try_new"
        )]
        unsafe {
            Payload::from_validated_bytes(self.payload_bytes())
        }
    }

    /// Validated metadata — one Arc share per `Some` over the underlying
    /// buffer.
    ///
    /// Returns `None` when the wire-level absent sentinel was present.
    #[must_use]
    pub fn metadata_value(&self) -> Option<Metadata> {
        self.metadata_bytes().map(|b| {
            // SAFETY: `from_validated_bytes` requires (1) `!bytes.is_empty()` and
            // (2) `bytes.len() <= MAX_METADATA_LEN`. Both invariants are
            // established by `try_new`: the `MetadataRangeEmpty` check rejects
            // an empty `Some(range)`, and `MetadataRangeTooLong` enforces the
            // cap on the range length.
            #[allow(
                unsafe_code,
                reason = "non-empty and length cap both established by try_new"
            )]
            unsafe {
                Metadata::from_validated_bytes(b)
            }
        })
    }

    /// The schema version widened to the kernel's [`Version`] for upcaster APIs.
    ///
    /// Total conversion — [`SchemaVersion`] is structurally nonzero.
    #[must_use]
    pub fn schema_version_as_version(&self) -> Version {
        Version::from(self.schema_version)
    }

    /// Wrap raw bytes in a synthetic envelope suitable for [`Decode`].
    ///
    /// Builds a fresh wire-format frame via [`crate::wire::encode_frame`] so the
    /// payload pointer lands on a 16-byte boundary. Use this when calling a
    /// [`Decode`](crate::codec::Decode) impl outside the cursor's normal frame
    /// buffer — snapshot decoding, upcaster post-transform decoding, codec
    /// round-trip tests.
    ///
    /// Reports `Version::INITIAL`, `GlobalSeq::INITIAL`, and
    /// `schema_version = SchemaVersion::INITIAL`. Most codecs ignore those
    /// fields; when they don't (or you're bridging an upcast back to a
    /// decode and need to preserve the original envelope's version
    /// triple), construct the envelope manually via [`try_new`](Self::try_new).
    ///
    /// # Errors
    ///
    /// Returns [`ForDecodeError`] if the value newtypes reject the inputs
    /// (oversize `event_type`/`payload`), the wire encode fails
    /// (`FrameLengthOverflow`), or the envelope `try_new` fails (range
    /// invariants).
    pub fn for_decode(event_type: &str, payload: &[u8]) -> Result<Self, ForDecodeError> {
        let et = EventType::from_bytes(Bytes::copy_from_slice(event_type.as_bytes()))?;
        let pl = Payload::from_bytes(Bytes::copy_from_slice(payload))?;
        let sv = SchemaVersion::INITIAL;
        let frame = crate::wire::encode_frame(GlobalSeq::INITIAL.as_u64(), sv, &et, &pl, None)?;
        Ok(Self::try_new(
            Version::INITIAL,
            GlobalSeq::INITIAL,
            frame.value,
            sv,
            frame.offsets.event_type,
            frame.offsets.payload,
            None,
        )?)
    }

    fn slice_range(&self, range: &Range<u32>) -> Bytes {
        self.value.slice(idx(range.start)..idx(range.end))
    }
}

const fn check_range(range: &Range<u32>, len: usize) -> Result<(), EnvelopeError> {
    if idx(range.end) > len || range.start > range.end {
        return Err(EnvelopeError::RangeOutOfBounds {
            start: range.start,
            end: range.end,
            len,
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code asserts exact values"
)]
mod tests {
    use super::*;
    use crate::value::MAX_EVENT_TYPE_LEN;
    use bytes::Bytes;
    use nexus::Version;

    #[test]
    fn pending_envelope_builds_with_metadata() {
        let env = pending_envelope(Version::INITIAL)
            .event_type("UserCreated")
            .payload(Bytes::from_static(b"payload-bytes"))
            .expect("valid payload")
            .with_metadata(Bytes::from_static(b"meta-bytes"))
            .expect("valid metadata");

        assert_eq!(env.event_type(), "UserCreated");
        assert_eq!(env.payload(), b"payload-bytes");
        assert_eq!(env.metadata(), Some(b"meta-bytes".as_slice()));
        assert_eq!(env.schema_version(), 1);
    }

    #[test]
    fn pending_envelope_builds_without_metadata() {
        let env = pending_envelope(Version::INITIAL)
            .event_type("X")
            .payload(Bytes::from_static(b"p"))
            .expect("valid payload")
            .build();

        assert_eq!(env.metadata(), None);
    }

    #[test]
    fn pending_envelope_typed_accessors_roundtrip() {
        let env = pending_envelope(Version::INITIAL)
            .event_type("UserCreated")
            .payload(Bytes::from_static(b"payload-bytes"))
            .expect("valid payload")
            .with_metadata(Bytes::from_static(b"meta-bytes"))
            .expect("valid metadata");

        assert_eq!(env.event_type_value().as_str(), "UserCreated");
        assert_eq!(env.payload_value().as_slice(), b"payload-bytes");
        assert_eq!(
            env.metadata_value().map(|m| m.as_slice().to_vec()),
            Some(b"meta-bytes".to_vec())
        );
        assert_eq!(env.schema_version_value().get(), 1);
    }

    #[test]
    fn pending_envelope_rejects_oversize_event_type() {
        let oversized = "x".repeat(MAX_EVENT_TYPE_LEN + 1);
        let err = pending_envelope(Version::INITIAL)
            .event_type_bytes(Bytes::from(oversized))
            .expect_err("oversized must be rejected");
        assert!(matches!(err, EnvelopeError::Value(_)));
    }

    #[test]
    fn persisted_envelope_accessors_return_views_into_value() {
        let value = Bytes::from_static(b"TYPEpayloadmeta");
        // ranges: event_type 0..4 ("TYPE"), payload 4..11 ("payload"), metadata 11..15 ("meta")
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            Some(11..15),
        )
        .expect("valid construction");

        assert_eq!(env.event_type(), "TYPE");
        assert_eq!(env.payload(), b"payload");
        assert_eq!(env.metadata(), Some(b"meta".as_slice()));
    }

    #[test]
    fn persisted_envelope_payload_bytes_shares_arc() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayload"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            None,
        )
        .unwrap();

        let payload = env.payload_bytes();
        assert_eq!(payload.as_ref(), b"payload");
    }

    #[test]
    fn persisted_envelope_rejects_range_past_buffer() {
        let value = Bytes::from_static(b"short");
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..100,
            None,
        )
        .expect_err("must reject out-of-bounds range");
        assert!(matches!(err, EnvelopeError::RangeOutOfBounds { .. }));
    }

    #[test]
    fn try_new_rejects_event_type_range_too_long() {
        let len = MAX_EVENT_TYPE_LEN + 1;
        let mut buf = vec![0u8; len];
        // Fill with valid UTF-8 (all-zeros is valid UTF-8 ASCII).
        buf[0] = b'A';
        let value = Bytes::from(buf);
        let too_long_end = u32::try_from(len).expect("len fits in u32 by construction");

        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            value,
            SchemaVersion::INITIAL,
            0..too_long_end,
            too_long_end..too_long_end,
            None,
        )
        .expect_err("event_type range > MAX_EVENT_TYPE_LEN must be rejected");

        let expected_actual = u32::try_from(len).expect("len fits in u32 by construction (above)");
        assert!(matches!(
            err,
            EnvelopeError::EventTypeRangeTooLong { actual, max }
                if actual == expected_actual && max == MAX_EVENT_TYPE_LEN
        ));
    }

    #[test]
    fn persisted_envelope_rejects_empty_metadata_range() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayload"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            Some(4..4),
        );
        assert!(matches!(env, Err(EnvelopeError::MetadataRangeEmpty)));
    }

    #[test]
    fn persisted_envelope_rejects_invalid_utf8_in_event_type() {
        let value = Bytes::from_static(&[0xFFu8, 0xFF, b'p', b'a', b'y']);
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            value,
            SchemaVersion::INITIAL,
            0..2,
            2..5,
            None,
        )
        .expect_err("must reject non-UTF-8 event_type");
        assert!(matches!(err, EnvelopeError::InvalidUtf8 { .. }));
    }

    #[test]
    fn persisted_envelope_event_type_value_returns_validated_value_newtype() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayload"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            None,
        )
        .expect("valid");

        let et = env.event_type_value();
        assert_eq!(et.as_str(), "TYPE");
    }

    #[test]
    fn persisted_envelope_payload_value_returns_validated_value_newtype() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayload"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            None,
        )
        .expect("valid");

        let p = env.payload_value();
        assert_eq!(p.as_slice(), b"payload");
    }

    #[test]
    fn persisted_envelope_metadata_value_returns_some_when_present() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayloadMETA"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            Some(11..15),
        )
        .expect("valid");

        let m = env.metadata_value().expect("present");
        assert_eq!(m.as_slice(), b"META");
    }

    #[test]
    fn persisted_envelope_metadata_value_returns_none_when_absent() {
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1).expect("nonzero"),
            Bytes::from_static(b"TYPEpayload"),
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            None,
        )
        .expect("valid");

        assert!(env.metadata_value().is_none());
    }

    #[cfg(feature = "import")]
    #[test]
    fn from_persisted_preserves_fields_drops_global_seq_zero_copy() {
        // A read-path envelope at version 7, global_seq 99, schema 3, with meta.
        let value = Bytes::from_static(b"TYPEpayloadmeta");
        let persisted = PersistedEnvelope::try_new(
            Version::new(7).expect("nonzero"),
            crate::store::GlobalSeq::new(99).expect("nonzero"),
            value,
            crate::value::SchemaVersion::from_u32(3).expect("nonzero"),
            0..4,
            4..11,
            Some(11..15),
        )
        .expect("valid");

        let pending = PendingEnvelope::from_persisted(&persisted);

        // Every field carried verbatim (global_seq has no home on the write path).
        assert_eq!(pending.version(), persisted.version());
        assert_eq!(pending.event_type(), "TYPE");
        assert_eq!(pending.payload(), b"payload");
        assert_eq!(pending.metadata(), Some(b"meta".as_slice()));
        assert_eq!(pending.schema_version(), 3);

        // Zero-copy: the rebuilt payload aliases the same backing allocation.
        assert!(
            std::ptr::eq(
                pending.payload_bytes().as_ptr(),
                persisted.payload().as_ptr()
            ),
            "from_persisted must reuse the Arc-shared payload, not deep-copy",
        );
    }
}

// ============================================================================
// Exhaustive PersistedEnvelope suite — migrated from the former PortableEvent
// tests (issue #145). PortableEvent's validator was a literal copy of
// PersistedEnvelope's, so its range/UTF-8/cap/zero-copy/fuzz tests apply here
// unchanged. The stream_id-specific tests were dropped (PersistedEnvelope has
// global_seq instead); the differential oracle was dropped (it compared the
// two now-unified validators). These cover the 4 cross-cutting categories:
// sequence/protocol, lifecycle, defensive boundary, and Send/Sync.
// ============================================================================
#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code")]
mod persisted_exhaustive_tests {
    use super::{EnvelopeError, PersistedEnvelope};
    use crate::store::GlobalSeq;
    use crate::value::{MAX_EVENT_TYPE_LEN, Metadata, SchemaVersion};
    use bytes::Bytes;
    use nexus::Version;
    use proptest::prelude::*;
    use static_assertions::assert_impl_all;
    use std::ops::Range;

    // ── helpers ─────────────────────────────────────────────────────────────

    fn v(n: u64) -> Version {
        Version::new(n).expect("test version must be nonzero")
    }

    fn sv(n: u32) -> SchemaVersion {
        SchemaVersion::from_u32(n).expect("test schema_version must be nonzero")
    }

    fn gs(n: u64) -> GlobalSeq {
        GlobalSeq::new(n).expect("test global_seq must be nonzero")
    }

    /// Assemble a contiguous `[event_type][payload][metadata?]` buffer and the
    /// matching ranges — the exact shape a decoder hands `try_new`.
    fn assemble(
        event_type: &[u8],
        payload: &[u8],
        metadata: Option<&[u8]>,
    ) -> (Bytes, Range<u32>, Range<u32>, Option<Range<u32>>) {
        let mut buf = Vec::new();
        buf.extend_from_slice(event_type);
        buf.extend_from_slice(payload);
        if let Some(m) = metadata {
            buf.extend_from_slice(m);
        }
        let et_end = u32::try_from(event_type.len()).expect("event_type len fits u32");
        let pl_end = et_end + u32::try_from(payload.len()).expect("payload len fits u32");
        let meta_range = metadata.map(|m| {
            let end = pl_end + u32::try_from(m.len()).expect("metadata len fits u32");
            pl_end..end
        });
        (Bytes::from(buf), 0..et_end, et_end..pl_end, meta_range)
    }

    /// Build a valid `PersistedEnvelope` from byte components.
    #[allow(
        clippy::too_many_arguments,
        reason = "test helper mirrors the 7-field PersistedEnvelope::try_new shape"
    )]
    fn build(
        version: Version,
        global_seq: GlobalSeq,
        schema: SchemaVersion,
        event_type: &[u8],
        payload: &[u8],
        metadata: Option<&[u8]>,
    ) -> PersistedEnvelope {
        let (value, et, pl, meta) = assemble(event_type, payload, metadata);
        PersistedEnvelope::try_new(version, global_seq, value, schema, et, pl, meta)
            .expect("components form a valid PersistedEnvelope")
    }

    // ── Category 4: linearizability / Send + Sync ───────────────────────────
    // PersistedEnvelope flows through `futures::Stream` items across `.await`
    // points and threads; Send + Sync is a structural contract.

    assert_impl_all!(PersistedEnvelope: Send, Sync, Clone, std::fmt::Debug);

    #[test]
    fn persisted_envelope_clones_across_thread_boundary() {
        let event = build(
            v(3),
            gs(99),
            sv(2),
            b"MoneyDeposited",
            b"payload-bytes",
            Some(b"meta"),
        );
        let moved = event.clone();
        // `std::thread::scope` (not the banned free `std::thread::spawn`) moves
        // the clone to another thread and joins it — proving `Send` in practice.
        let (gseq, ver, schema, etype, payload, meta) = std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    (
                        moved.global_seq(),
                        moved.version(),
                        moved.schema_version(),
                        moved.event_type().to_owned(),
                        moved.payload().to_vec(),
                        moved.metadata().map(<[u8]>::to_vec),
                    )
                })
                .join()
                .expect("worker thread must not panic")
        });

        assert_eq!(gseq, gs(99));
        assert_eq!(ver, v(3));
        assert_eq!(schema, 2);
        assert_eq!(etype, "MoneyDeposited");
        assert_eq!(payload, b"payload-bytes");
        assert_eq!(meta, Some(b"meta".to_vec()));
        // Original still usable after the clone left for another thread.
        assert_eq!(event.event_type(), "MoneyDeposited");
    }

    // ── Category 1: sequence / protocol ─────────────────────────────────────
    // Multi-step interaction on ONE object: repeated and interleaved accessor
    // calls must be deterministic and mutually consistent.

    #[test]
    fn repeated_accessor_calls_are_consistent() {
        let event = build(v(9), gs(5), sv(4), b"TYPE", b"payload", Some(b"meta"));

        // Same accessor twice → identical.
        assert_eq!(event.event_type(), event.event_type());
        assert_eq!(event.payload(), event.payload());
        assert_eq!(event.metadata(), event.metadata());

        // Interleave borrowed and owned views → still consistent.
        assert_eq!(
            event.event_type().as_bytes(),
            event.event_type_bytes().as_ref()
        );
        assert_eq!(event.payload(), event.payload_bytes().as_ref());
        assert_eq!(event.metadata(), event.metadata_bytes().as_deref());

        // Borrowed vs validated-newtype views agree.
        assert_eq!(event.event_type(), event.event_type_value().as_str());
        assert_eq!(event.payload(), event.payload_value().as_slice());
        assert_eq!(
            event.metadata(),
            event.metadata_value().as_ref().map(Metadata::as_slice),
        );

        // Scalars are stable across repeated reads.
        assert_eq!(event.version(), v(9));
        assert_eq!(event.version(), event.version());
        assert_eq!(event.global_seq(), gs(5));
        assert_eq!(event.global_seq(), event.global_seq());
        assert_eq!(event.schema_version(), 4);
        assert_eq!(event.schema_version_value(), sv(4));
    }

    // ── Category 2: lifecycle (build / clone / access-after-clone) ───────────

    #[test]
    fn clone_is_field_for_field_equal_view() {
        let original = build(
            v(42),
            gs(7),
            sv(7),
            b"OrderPlaced",
            b"the-payload",
            Some(b"the-meta"),
        );
        let cloned = original.clone();

        assert_eq!(cloned.version(), original.version());
        assert_eq!(cloned.global_seq(), original.global_seq());
        assert_eq!(cloned.schema_version(), original.schema_version());
        assert_eq!(
            cloned.schema_version_value(),
            original.schema_version_value()
        );
        assert_eq!(cloned.event_type(), original.event_type());
        assert_eq!(cloned.payload(), original.payload());
        assert_eq!(cloned.metadata(), original.metadata());
        assert_eq!(
            cloned.metadata_value().map(|m| m.as_slice().to_vec()),
            original.metadata_value().map(|m| m.as_slice().to_vec()),
        );
    }

    #[test]
    fn clone_shares_the_same_backing_buffer() {
        // A clone is one Arc refcount inc — owned views from both clones must
        // point at the SAME allocation (zero-copy), not a deep copy.
        let original = build(v(1), gs(1), sv(1), b"TYPE", b"payload", None);
        let cloned = original.clone();

        let from_original = original.payload_bytes();
        let from_clone = cloned.payload_bytes();
        assert!(
            std::ptr::eq(from_original.as_ptr(), from_clone.as_ptr()),
            "clone must share the parent buffer, not deep-copy it",
        );
        assert_eq!(from_original.as_ref(), from_clone.as_ref());
    }

    // ── Category 3: defensive boundary (reject upstream-guarantee violations) ─

    #[test]
    fn rejects_inverted_range_start_after_end() {
        let value = Bytes::from_static(b"TYPEpayload");
        // Built from variables so the inverted range is not a compile-time
        // literal (which `reversed_empty_ranges` would reject before runtime).
        let (bad_start, bad_end) = (7u32, 2u32);
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            0..4,
            bad_start..bad_end,
            None,
        )
        .expect_err("start > end must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::RangeOutOfBounds {
                start: 7,
                end: 2,
                ..
            }
        ));
    }

    #[test]
    fn event_type_cap_uses_range_length_not_endpoint_sum() {
        // Mutation-testing pin (cargo-mutants surfaced this gap): the cap check
        // is on `event_type_range.end - event_type_range.start`, NOT `+`. Every
        // other test starts the event_type range at 0, where `end - 0 == end +
        // 0`, so none can tell the operators apart. Here start > 0 and the
        // length (end - start = 30_000) is WITHIN the cap, while the endpoint
        // sum (end + start = 110_000) EXCEEDS it — so a `- → +` mutant would
        // wrongly reject this valid event, and the `.expect` below catches it.
        let start: u32 = 40_000;
        let end: u32 = 70_000;
        let buf_len = usize::try_from(end).expect("70_000 fits usize");
        let value = Bytes::from(vec![b'A'; buf_len]);
        let event = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            start..end,
            0..0,
            None,
        )
        .expect("event_type length 30_000 is within MAX_EVENT_TYPE_LEN (65_535)");
        assert_eq!(event.event_type().len(), 30_000);
    }

    // Each of the three ranges is bounds-checked independently. The payload
    // path is covered in the outer module; these pin the event_type and
    // metadata paths so a regression deleting either check is caught.

    #[test]
    fn rejects_event_type_range_past_buffer_end() {
        let value = Bytes::from_static(b"short");
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            // event_type is the FIRST range checked; end 100 > len 5.
            0..100,
            0..0,
            None,
        )
        .expect_err("event_type range past buffer must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::RangeOutOfBounds {
                start: 0,
                end: 100,
                len: 5
            }
        ));
    }

    #[test]
    fn rejects_metadata_range_past_buffer_end() {
        let value = Bytes::from_static(b"TYPEpayload");
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            // event_type + payload in-bounds so the metadata check is reached.
            0..4,
            4..11,
            Some(11..100),
        )
        .expect_err("metadata range past buffer must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::RangeOutOfBounds {
                start: 11,
                end: 100,
                len: 11
            }
        ));
    }

    #[test]
    fn rejects_inverted_metadata_range() {
        let value = Bytes::from_static(b"TYPEpayload");
        let (bad_start, bad_end) = (9u32, 5u32);
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            Some(bad_start..bad_end),
        )
        .expect_err("metadata start > end must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::RangeOutOfBounds {
                start: 9,
                end: 5,
                ..
            }
        ));
    }

    // ── absence-of-invariant pins: independent (possibly overlapping) ranges ─

    #[test]
    fn ranges_are_independent_and_may_overlap() {
        // try_new enforces NO disjointness/contiguity. All three windows can
        // point at the SAME 4 bytes. A future change adding a disjointness
        // invariant is a conscious break, caught here.
        let value = Bytes::from_static(b"TYPE");
        let event = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            0..4,
            0..4,
            Some(0..4),
        )
        .expect("overlapping ranges are structurally permitted");
        assert_eq!(event.event_type(), "TYPE");
        assert_eq!(event.payload(), b"TYPE");
        assert_eq!(event.metadata(), Some(b"TYPE".as_slice()));
    }

    #[test]
    fn empty_payload_range_is_accepted_unlike_empty_metadata() {
        // Deliberate asymmetry mirrored from the value newtypes: an empty
        // payload is a legal event shape (marker event), an empty metadata
        // Some is not (use None).
        let value = Bytes::from_static(b"TYPE");
        let event = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..4,
            None,
        )
        .expect("empty payload range is accepted");
        assert_eq!(event.payload(), b"");
        assert_eq!(event.payload_value().as_slice(), b"");
    }

    // ── metadata None vs Some across every metadata accessor ────────────────

    #[test]
    fn metadata_absent_is_none_everywhere() {
        let event = build(v(1), gs(1), sv(1), b"TYPE", b"payload", None);
        assert!(event.metadata().is_none());
        assert!(event.metadata_bytes().is_none());
        assert!(event.metadata_value().is_none());
    }

    #[test]
    fn metadata_present_threads_through_every_accessor() {
        let event = build(v(1), gs(1), sv(1), b"TYPE", b"payload", Some(b"META"));
        assert_eq!(event.metadata(), Some(b"META".as_slice()));
        assert_eq!(event.metadata_bytes().expect("some").as_ref(), b"META");
        assert_eq!(event.metadata_value().expect("some").as_slice(), b"META");
    }

    // ── empty event_type accepted (no minimum-length invariant) ─────────────

    #[test]
    fn empty_event_type_is_accepted() {
        let event = build(v(1), gs(1), sv(1), b"", b"payload", None);
        assert_eq!(event.event_type(), "");
        assert_eq!(event.event_type_value().as_str(), "");
        assert_eq!(event.payload(), b"payload");
    }

    // ── boundary scalars carried verbatim (version, global_seq, schema) ──────

    #[test]
    fn boundary_version_global_seq_and_schema_version_carried_verbatim() {
        let event = build(
            v(u64::MAX),
            gs(u64::MAX),
            sv(u32::MAX),
            b"TYPE",
            b"payload",
            None,
        );
        assert_eq!(event.version().as_u64(), u64::MAX);
        assert_eq!(event.global_seq().as_u64(), u64::MAX);
        assert_eq!(event.schema_version(), u32::MAX);
        assert_eq!(event.schema_version_value().get(), u32::MAX);
    }

    // ── zero-copy aliasing: owned views share the one Arc buffer ────────────

    #[test]
    fn owned_views_alias_the_single_backing_buffer() {
        let (value, et, pl, meta) = assemble(b"TYPE", b"payload", Some(b"meta"));
        let base = value.clone(); // shares the same allocation as `value`
        let event = PersistedEnvelope::try_new(
            Version::INITIAL,
            gs(1),
            value,
            SchemaVersion::INITIAL,
            et,
            pl,
            meta,
        )
        .expect("valid");

        let et_bytes = event.event_type_bytes();
        let pl_bytes = event.payload_bytes();
        let meta_bytes = event.metadata_bytes().expect("metadata present");

        // Each owned view points into `base` at exactly its range offset →
        // proves no copy AND that all three share the one buffer.
        assert!(
            std::ptr::eq(et_bytes.as_ptr(), base.as_ptr().wrapping_add(0)),
            "event_type_bytes must alias buffer offset 0",
        );
        assert!(
            std::ptr::eq(pl_bytes.as_ptr(), base.as_ptr().wrapping_add(4)),
            "payload_bytes must alias buffer offset 4",
        );
        assert!(
            std::ptr::eq(meta_bytes.as_ptr(), base.as_ptr().wrapping_add(11)),
            "metadata_bytes must alias buffer offset 11",
        );
        // The value-newtype path shares the same buffer too.
        assert!(std::ptr::eq(
            event.event_type_value().as_bytes().as_ptr(),
            base.as_ptr().wrapping_add(0),
        ));
        assert!(std::ptr::eq(
            event.payload_value().as_slice().as_ptr(),
            base.as_ptr().wrapping_add(4),
        ));
    }

    // ── Property: random valid components round-trip through every accessor ──

    fn boundary_u64() -> impl Strategy<Value = u64> {
        prop_oneof![
            Just(1u64),
            Just(2u64),
            Just(u64::MAX - 1),
            Just(u64::MAX),
            1u64..=u64::MAX,
        ]
    }

    fn boundary_u32_nonzero() -> impl Strategy<Value = u32> {
        prop_oneof![
            Just(1u32),
            Just(2u32),
            Just(u32::MAX - 1),
            Just(u32::MAX),
            1u32..=u32::MAX,
        ]
    }

    fn event_type_bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
        // ASCII 'A'..='Z' guarantees valid UTF-8; lengths include the empty,
        // unit, interior, and exact-cap boundaries.
        prop_oneof![
            Just(0usize),
            Just(1usize),
            Just(255usize),
            Just(MAX_EVENT_TYPE_LEN),
        ]
        .prop_flat_map(|n| proptest::collection::vec(b'A'..=b'Z', n))
    }

    fn payload_bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![Just(0usize), Just(1usize), Just(1024usize)]
            .prop_flat_map(|n| proptest::collection::vec(any::<u8>(), n))
    }

    fn metadata_bytes_strategy() -> impl Strategy<Value = Option<Vec<u8>>> {
        prop_oneof![
            Just(None),
            Just(Some(vec![0u8])),
            (1usize..=2048)
                .prop_flat_map(|n| proptest::collection::vec(any::<u8>(), n))
                .prop_map(Some),
        ]
    }

    proptest! {
        #[test]
        fn persisted_envelope_roundtrips_every_accessor(
            event_type in event_type_bytes_strategy(),
            payload in payload_bytes_strategy(),
            metadata in metadata_bytes_strategy(),
            version_raw in boundary_u64(),
            global_seq_raw in boundary_u64(),
            schema_raw in boundary_u32_nonzero(),
        ) {
            let version = v(version_raw);
            let global_seq = gs(global_seq_raw);
            let schema = sv(schema_raw);
            let event = build(
                version,
                global_seq,
                schema,
                &event_type,
                &payload,
                metadata.as_deref(),
            );

            // Scalars verbatim.
            prop_assert_eq!(event.version(), version);
            prop_assert_eq!(event.version().as_u64(), version_raw);
            prop_assert_eq!(event.global_seq(), global_seq);
            prop_assert_eq!(event.global_seq().as_u64(), global_seq_raw);
            prop_assert_eq!(event.schema_version(), schema_raw);
            prop_assert_eq!(event.schema_version_value(), schema);

            // event_type across all three views. Owned views bound to locals
            // first so the backing Bytes/newtype outlives the borrow.
            let et_bytes = event.event_type_bytes();
            let et_value = event.event_type_value();
            prop_assert_eq!(event.event_type().as_bytes(), event_type.as_slice());
            prop_assert_eq!(et_bytes.as_ref(), event_type.as_slice());
            prop_assert_eq!(et_value.as_bytes(), event_type.as_slice());

            // payload across all three views.
            let pl_bytes = event.payload_bytes();
            let pl_value = event.payload_value();
            prop_assert_eq!(event.payload(), payload.as_slice());
            prop_assert_eq!(pl_bytes.as_ref(), payload.as_slice());
            prop_assert_eq!(pl_value.as_slice(), payload.as_slice());

            // metadata: presence and bytes consistent across all three views.
            if let Some(ref m) = metadata {
                prop_assert_eq!(event.metadata(), Some(m.as_slice()));
                let meta_bytes = event.metadata_bytes().expect("some");
                prop_assert_eq!(meta_bytes.as_ref(), m.as_slice());
                let meta_value = event.metadata_value().expect("some");
                prop_assert_eq!(meta_value.as_slice(), m.as_slice());
            } else {
                prop_assert!(event.metadata().is_none());
                prop_assert!(event.metadata_bytes().is_none());
                prop_assert!(event.metadata_value().is_none());
            }
        }
    }

    // ── Adversarial robustness: try_new is fed UNTRUSTED decoder output ──────
    //
    // On the read path a corrupt/adversarial row reaches try_new. It must be
    // panic-free for ANY (value, ranges) and either return Ok or a KNOWN
    // EnvelopeError — never a panic, and never an Ok whose unsafe accessors are
    // unsound. The exhaustive Err match has no catch-all, so a new variant is a
    // compile error here — pinning the rejection surface. Run under Miri to
    // prove the unsafe accessors' preconditions hold for fuzzed input.

    fn arbitrary_range() -> impl Strategy<Value = Range<u32>> {
        (0u32..=260, 0u32..=260).prop_map(|(start, end)| start..end)
    }

    fn arbitrary_optional_range() -> impl Strategy<Value = Option<Range<u32>>> {
        prop_oneof![Just(None), arbitrary_range().prop_map(Some)]
    }

    proptest! {
        #[test]
        fn try_new_never_panics_and_accepted_events_have_sound_accessors(
            value_bytes in proptest::collection::vec(any::<u8>(), 0..256),
            event_type_range in arbitrary_range(),
            payload_range in arbitrary_range(),
            metadata_range in arbitrary_optional_range(),
            version_raw in boundary_u64(),
            global_seq_raw in boundary_u64(),
            schema_raw in boundary_u32_nonzero(),
        ) {
            let result = PersistedEnvelope::try_new(
                v(version_raw),
                gs(global_seq_raw),
                Bytes::from(value_bytes),
                sv(schema_raw),
                event_type_range,
                payload_range,
                metadata_range,
            );

            match result {
                Ok(event) => {
                    // Every accessor must be callable without panic/UB. The
                    // event_type() / *_value() paths run `unsafe`
                    // from_utf8_unchecked / from_validated_bytes — under Miri
                    // this proves their preconditions held for fuzzed input.
                    let et = event.event_type();
                    prop_assert!(std::str::from_utf8(et.as_bytes()).is_ok());
                    let _ = event.payload();
                    let _ = event.metadata();
                    let _ = event.event_type_value();
                    let _ = event.payload_value();
                    let _ = event.metadata_value();
                    let _ = event.event_type_bytes();
                    let _ = event.payload_bytes();
                    let _ = event.metadata_bytes();
                }
                Err(err) => {
                    // Exhaustive — no `_` arm. A new EnvelopeError variant must
                    // be classified here, not silently absorbed.
                    match err {
                        EnvelopeError::RangeOutOfBounds { .. }
                        | EnvelopeError::InvalidUtf8 { .. }
                        | EnvelopeError::EventTypeRangeTooLong { .. }
                        | EnvelopeError::MetadataRangeTooLong { .. }
                        | EnvelopeError::MetadataRangeEmpty
                        | EnvelopeError::Value(_) => {}
                    }
                }
            }
        }
    }
}
