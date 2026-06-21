//! Transport-view of a persisted event — the unit of export/import.
//!
//! [`PortableEvent`] is the third member of the envelope family, completing
//! [`PendingEnvelope`](crate::envelope::PendingEnvelope) (write-view) and
//! [`PersistedEnvelope`](crate::envelope::PersistedEnvelope) (read-view) with
//! a **transport-view**. It is a faithful copy of `PersistedEnvelope`'s
//! representation — one owned [`Bytes`] buffer plus cached `Range<u32>`
//! offsets — with exactly two differences, both dictated by what survives a
//! trip to another store:
//!
//! - **drops `global_seq`** — store-local, non-portable; meaningless once the
//!   event leaves its origin store.
//! - **adds `stream_id`** — the producer's own stream id, which on the read
//!   path lives in the adapter key (not the envelope) and so must be carried
//!   explicitly once the event travels.
//!
//! Everything else — `version`, `schema_version`, `event_type`, `metadata`,
//! `payload` — is carried verbatim. The `payload` rides as opaque bytes; the
//! producer's codec choice is never re-encoded.
//!
//! Construction validates the same range/UTF-8/cap invariants as
//! `PersistedEnvelope` and reuses [`EnvelopeError`].

use std::ops::Range;

use bytes::Bytes;
use nexus::Version;

use crate::envelope::EnvelopeError;
use crate::value::{
    EventType, MAX_EVENT_TYPE_LEN, MAX_METADATA_LEN, Metadata, Payload, SchemaVersion,
};

/// Cast `u32` index to `usize` for slice indexing.
///
/// `u32 as usize` is lossless on all platforms Nexus supports (32-bit+).
#[allow(
    clippy::as_conversions,
    reason = "u32→usize is lossless on all Nexus target platforms (32-bit+)"
)]
#[inline]
const fn idx(n: u32) -> usize {
    n as usize
}

/// Transport-view of one event: a `Bytes` buffer + cached offsets, carrying
/// the producer `stream_id` and dropping the store-local `global_seq`.
///
/// Cheap to clone (one Arc refcount + range/Bytes copies), no lifetime
/// parameter, so it flows through `futures::Stream` items without bridging.
#[derive(Debug, Clone)]
pub struct PortableEvent {
    stream_id: Bytes,
    version: Version,
    schema_version: SchemaVersion,
    value: Bytes,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
}

impl PortableEvent {
    /// Construct from decoded transport data, validating ranges and UTF-8.
    ///
    /// Mirrors [`PersistedEnvelope::try_new`](crate::envelope::PersistedEnvelope::try_new):
    /// `stream_id` replaces `global_seq`; every other invariant is identical.
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
        reason = "all 7 fields are required to construct a validated PortableEvent; \
                  a builder would add indirection with no type-safety benefit here"
    )]
    pub fn try_new(
        stream_id: Bytes,
        version: Version,
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

        // Structural per-field caps — owned upstream by the value newtypes;
        // mirroring them here makes the `*_value()` accessors sound without
        // re-validation.
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
            stream_id,
            version,
            schema_version,
            value,
            event_type_range,
            payload_range,
            metadata_range,
        })
    }

    /// The producer's stream id bytes (the event's origin stream).
    #[must_use]
    pub fn stream_id(&self) -> &[u8] {
        &self.stream_id
    }

    /// Owned `Bytes` view of the producer stream id — one Arc refcount inc.
    #[must_use]
    pub fn stream_id_bytes(&self) -> Bytes {
        self.stream_id.clone()
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
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
        // SAFETY: `from_validated_bytes` requires valid UTF-8 and
        // `len <= MAX_EVENT_TYPE_LEN`; both established by `try_new`.
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
        // SAFETY: `payload_range` validated against `value.len()` in
        // `try_new`; the range is `Range<u32>`, so length <= MAX_PAYLOAD_LEN.
        #[allow(unsafe_code, reason = "size invariant established at try_new")]
        unsafe {
            Payload::from_validated_bytes(self.payload_bytes())
        }
    }

    /// Validated metadata — one Arc share per `Some` over the underlying
    /// buffer. Returns `None` when absent.
    #[must_use]
    pub fn metadata_value(&self) -> Option<Metadata> {
        self.metadata_bytes().map(|b| {
            // SAFETY: `from_validated_bytes` requires non-empty and
            // `len <= MAX_METADATA_LEN`; both established by `try_new`.
            #[allow(
                unsafe_code,
                reason = "non-empty and length cap both established by try_new"
            )]
            unsafe {
                Metadata::from_validated_bytes(b)
            }
        })
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
    use proptest::prelude::*;
    use static_assertions::assert_impl_all;

    // ── helpers ─────────────────────────────────────────────────────────────

    fn v(n: u64) -> Version {
        Version::new(n).expect("test version must be nonzero")
    }

    fn sv(n: u32) -> SchemaVersion {
        SchemaVersion::from_u32(n).expect("test schema_version must be nonzero")
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

    /// Build a valid `PortableEvent` from byte components, with fixed
    /// `stream_id`/`version`/`schema_version` (overridable per-test).
    #[allow(
        clippy::too_many_arguments,
        reason = "test helper mirrors the 7-field PortableEvent::try_new shape"
    )]
    fn build(
        stream_id: &[u8],
        version: Version,
        schema: SchemaVersion,
        event_type: &[u8],
        payload: &[u8],
        metadata: Option<&[u8]>,
    ) -> PortableEvent {
        let (value, et, pl, meta) = assemble(event_type, payload, metadata);
        PortableEvent::try_new(
            Bytes::copy_from_slice(stream_id),
            version,
            value,
            schema,
            et,
            pl,
            meta,
        )
        .expect("components form a valid PortableEvent")
    }

    // ── Category 4: linearizability / Send + Sync ───────────────────────────
    // PortableEvent flows through `futures::Stream` items across `.await`
    // points and (potentially) threads; Send + Sync is a structural contract.

    assert_impl_all!(PortableEvent: Send, Sync, Clone, std::fmt::Debug);

    #[test]
    fn portable_event_clones_across_thread_boundary() {
        let event = build(
            b"acct-7",
            v(3),
            sv(2),
            b"MoneyDeposited",
            b"payload-bytes",
            Some(b"meta"),
        );
        let moved = event.clone();
        // `std::thread::scope` (not the banned free `std::thread::spawn`) moves
        // the clone to another thread and joins it — proving `PortableEvent` is
        // `Send` in practice, not just by static assertion.
        let (sid, ver, schema, etype, payload, meta) = std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    (
                        moved.stream_id().to_vec(),
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

        assert_eq!(sid, b"acct-7");
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
        let event = build(b"sid", v(9), sv(4), b"TYPE", b"payload", Some(b"meta"));

        // Same accessor twice → identical.
        assert_eq!(event.event_type(), event.event_type());
        assert_eq!(event.payload(), event.payload());
        assert_eq!(event.metadata(), event.metadata());
        assert_eq!(event.stream_id(), event.stream_id());

        // Interleave borrowed and owned views → still consistent.
        assert_eq!(
            event.event_type().as_bytes(),
            event.event_type_bytes().as_ref()
        );
        assert_eq!(event.payload(), event.payload_bytes().as_ref());
        assert_eq!(event.metadata(), event.metadata_bytes().as_deref(),);
        assert_eq!(event.stream_id(), event.stream_id_bytes().as_ref());

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
        assert_eq!(event.schema_version(), 4);
        assert_eq!(event.schema_version_value(), sv(4));
    }

    // ── Category 2: lifecycle (build / clone / access-after-clone) ───────────

    #[test]
    fn clone_is_field_for_field_equal_view() {
        let original = build(
            b"origin-stream",
            v(42),
            sv(7),
            b"OrderPlaced",
            b"the-payload",
            Some(b"the-meta"),
        );
        let cloned = original.clone();

        assert_eq!(cloned.stream_id(), original.stream_id());
        assert_eq!(cloned.version(), original.version());
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
        let original = build(b"sid", v(1), sv(1), b"TYPE", b"payload", None);
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
    fn rejects_payload_range_past_buffer_end() {
        let value = Bytes::from_static(b"TYPEshort");
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..100,
            None,
        )
        .expect_err("payload range past buffer must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::RangeOutOfBounds {
                start: 4,
                end: 100,
                len: 9
            }
        ));
    }

    #[test]
    fn rejects_inverted_range_start_after_end() {
        let value = Bytes::from_static(b"TYPEpayload");
        // Built from variables so the inverted range is not a compile-time
        // literal (which `reversed_empty_ranges` would reject before runtime).
        let (bad_start, bad_end) = (7u32, 2u32);
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
            value,
            SchemaVersion::INITIAL,
            // event_type 0..4 ok; payload start>end is the violation.
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
    fn rejects_invalid_utf8_event_type() {
        let value = Bytes::from_static(&[0xFFu8, 0xFF, b'p', b'a', b'y']);
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
            value,
            SchemaVersion::INITIAL,
            0..2,
            2..5,
            None,
        )
        .expect_err("non-UTF-8 event_type must be rejected");
        assert!(matches!(
            err,
            EnvelopeError::InvalidUtf8 {
                start: 0,
                end: 2,
                ..
            }
        ));
    }

    #[test]
    fn rejects_event_type_range_longer_than_cap() {
        // Buffer must actually be MAX+1 bytes so the range is in-bounds and the
        // length cap (not RangeOutOfBounds) is the rejection reason.
        let len = MAX_EVENT_TYPE_LEN + 1;
        let value = Bytes::from(vec![b'A'; len]);
        let end = u32::try_from(len).expect("MAX_EVENT_TYPE_LEN + 1 fits u32");
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
            value,
            SchemaVersion::INITIAL,
            0..end,
            end..end,
            None,
        )
        .expect_err("event_type range > MAX_EVENT_TYPE_LEN must be rejected");
        let expected_actual = u32::try_from(len).expect("fits u32");
        assert!(matches!(
            err,
            EnvelopeError::EventTypeRangeTooLong { actual, max }
                if actual == expected_actual && max == MAX_EVENT_TYPE_LEN
        ));
    }

    #[test]
    fn rejects_empty_metadata_range_with_some() {
        let value = Bytes::from_static(b"TYPEpayload");
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
            value,
            SchemaVersion::INITIAL,
            0..4,
            4..11,
            Some(4..4),
        )
        .expect_err("Some(empty range) must be rejected; absent metadata is None");
        assert!(matches!(err, EnvelopeError::MetadataRangeEmpty));
    }

    // Each of the three ranges is bounds-checked independently (three separate
    // `check_range` calls). The payload path is covered above; these pin the
    // event_type and metadata paths so a regression deleting either check is
    // caught.

    #[test]
    fn rejects_event_type_range_past_buffer_end() {
        let value = Bytes::from_static(b"short");
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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
        // Built from variables so the inverted range is not a compile-time
        // literal (`reversed_empty_ranges` would reject it before runtime).
        let (bad_start, bad_end) = (9u32, 5u32);
        let err = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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
        // Frozen-surface pin: try_new enforces NO disjointness/contiguity. All
        // three windows can point at the SAME 4 bytes. A future change adding a
        // disjointness invariant is a conscious break, caught here.
        let value = Bytes::from_static(b"TYPE");
        let event = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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
        // Some is not (use None). `rejects_empty_metadata_range_with_some`
        // pins the other half.
        let value = Bytes::from_static(b"TYPE");
        let event = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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

    // NOTE: `MetadataRangeTooLong` is structurally unreachable in a unit test.
    // To trip it, `metadata_range.len() > MAX_METADATA_LEN (== u32::MAX - 1)`,
    // but `check_range` fires `RangeOutOfBounds` first unless the backing
    // buffer is itself > 4 GiB. The cap is mirrored from the `Metadata`
    // newtype (see value.rs, which makes the same impracticality note for the
    // 4 GiB boundary). Covered structurally, not by allocation.

    // ── stream_id carried verbatim; global_seq structurally absent ──────────

    #[test]
    fn stream_id_carried_verbatim_including_empty() {
        // Non-empty: bytes echoed exactly, both borrowed and owned views.
        let event = build(b"task-123", v(1), sv(1), b"E", b"", None);
        assert_eq!(event.stream_id(), b"task-123");
        assert_eq!(event.stream_id_bytes().as_ref(), b"task-123");

        // Empty stream_id is structurally legal (no minimum-length invariant).
        let empty = build(b"", v(1), sv(1), b"E", b"", None);
        assert_eq!(empty.stream_id(), b"");
        assert!(empty.stream_id_bytes().is_empty());
    }

    #[test]
    fn portable_event_exposes_no_global_seq() {
        // PortableEvent deliberately drops the store-local `global_seq`. There
        // is no runtime value to assert, so this pins the contract two ways:
        // (1) it documents the intent at the test layer, and (2) the line
        // below is a COMPILE-time tripwire — uncommenting it must fail to
        // build, because no such accessor exists. If a `global_seq()` method
        // is ever added, this test's comment flags the frozen-surface break.
        let event = build(b"sid", v(5), sv(1), b"E", b"p", None);
        // let _ = event.global_seq(); // <-- must NOT compile
        assert_eq!(event.version(), v(5));
    }

    // ── zero-copy aliasing: owned views share the one Arc buffer ────────────

    #[test]
    fn owned_views_alias_the_single_backing_buffer() {
        let (value, et, pl, meta) = assemble(b"TYPE", b"payload", Some(b"meta"));
        let base = value.clone(); // shares the same allocation as `value`
        let event = PortableEvent::try_new(
            Bytes::from_static(b"sid"),
            Version::INITIAL,
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

    // ── metadata None vs Some across every metadata accessor ────────────────

    #[test]
    fn metadata_absent_is_none_everywhere() {
        let event = build(b"sid", v(1), sv(1), b"TYPE", b"payload", None);
        assert!(event.metadata().is_none());
        assert!(event.metadata_bytes().is_none());
        assert!(event.metadata_value().is_none());
    }

    #[test]
    fn metadata_present_threads_through_every_accessor() {
        let event = build(b"sid", v(1), sv(1), b"TYPE", b"payload", Some(b"META"));
        assert_eq!(event.metadata(), Some(b"META".as_slice()));
        assert_eq!(event.metadata_bytes().expect("some").as_ref(), b"META");
        assert_eq!(event.metadata_value().expect("some").as_slice(), b"META",);
    }

    // ── empty event_type accepted (no minimum-length invariant) ─────────────

    #[test]
    fn empty_event_type_is_accepted() {
        // Deliberate frozen-surface pin: unlike metadata, an empty event_type
        // (range 0..0) passes — there is no min-length check. A future change
        // that tightens this is a conscious break, caught here.
        let event = build(b"sid", v(1), sv(1), b"", b"payload", None);
        assert_eq!(event.event_type(), "");
        assert_eq!(event.event_type_value().as_str(), "");
        assert_eq!(event.payload(), b"payload");
    }

    // ── boundary scalars carried verbatim ───────────────────────────────────

    #[test]
    fn boundary_version_and_schema_version_carried_verbatim() {
        let event = build(b"sid", v(u64::MAX), sv(u32::MAX), b"TYPE", b"payload", None);
        assert_eq!(event.version().as_u64(), u64::MAX);
        assert_eq!(event.schema_version(), u32::MAX);
        assert_eq!(event.schema_version_value().get(), u32::MAX);
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
        // None, the minimum non-empty (len 1), and an interior length. The
        // 4 GiB metadata cap is untestable (see the MetadataRangeTooLong note).
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
        fn portable_event_roundtrips_every_accessor(
            stream_id in proptest::collection::vec(any::<u8>(), 0..64),
            event_type in event_type_bytes_strategy(),
            payload in payload_bytes_strategy(),
            metadata in metadata_bytes_strategy(),
            version_raw in boundary_u64(),
            schema_raw in boundary_u32_nonzero(),
        ) {
            let version = v(version_raw);
            let schema = sv(schema_raw);
            let event = build(
                &stream_id,
                version,
                schema,
                &event_type,
                &payload,
                metadata.as_deref(),
            );

            // Scalars verbatim. Owned views are bound to locals first so the
            // backing `Bytes`/newtype outlives `prop_assert_eq!`'s borrow.
            let stream_id_owned = event.stream_id_bytes();
            prop_assert_eq!(event.stream_id(), stream_id.as_slice());
            prop_assert_eq!(stream_id_owned.as_ref(), stream_id.as_slice());
            prop_assert_eq!(event.version(), version);
            prop_assert_eq!(event.version().as_u64(), version_raw);
            prop_assert_eq!(event.schema_version(), schema_raw);
            prop_assert_eq!(event.schema_version_value(), schema);

            // event_type across all three views.
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
}
