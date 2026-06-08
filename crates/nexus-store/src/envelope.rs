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
        if usize::try_from(et_range_len).unwrap_or(usize::MAX) > MAX_EVENT_TYPE_LEN {
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
            if usize::try_from(meta_range_len).unwrap_or(usize::MAX) > MAX_METADATA_LEN {
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
}
