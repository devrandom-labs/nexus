//! Wire-format frame builder shared by `nexus-fjall` and `nexus-store::testing`.
//!
//! One canonical implementation of the on-disk frame format: a fixed
//! header, then event-type bytes, optional metadata bytes, alignment
//! padding, and finally payload. The padding makes the payload pointer
//! 16-byte aligned in the resulting [`Bytes`] buffer, which is a
//! wire-format invariant — every adapter must use [`encode_frame`] to
//! encode frames, every decoder may rely on the alignment.
//!
//! Layout:
//!
//! ```text
//! [u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len]
//! [u32 LE meta_len][event_type bytes][metadata bytes if any]
//! [padding zero-bytes][payload bytes]
//! ```
//!
//! `meta_len == u32::MAX` is the absent-metadata sentinel
//! (distinguishes `None` from `Some(empty)`).
//!
//! Pipeline: [`encode_frame`] is `execute(plan(...)?)` — `plan` does all
//! the validation and layout math (fallible), `execute` does the buffer
//! fill (infallible). Each stage is independently testable.
//!
//! # Implicit couplings (deliberate, but worth knowing)
//!
//! - **Payload length is not stored.** It's derived as
//!   `value.len() - (header + event_type + metadata + padding)`. Saves
//!   four bytes per row but means truncation that lops bytes off the
//!   *end* of a frame is structurally undetectable here. Storage layers
//!   that wrap [`encode_frame`] output (fjall, snapshots) own
//!   value-integrity guarantees.
//! - **Decode recomputes the padding** via the same [`align_padding`]
//!   formula the encoder used; there is no padding-length field. Any
//!   future change to the alignment formula is a wire break — both
//!   sides must change together.
//!
//! # Validation symmetry
//!
//! [`encode_frame`] rejects `schema_version == 0` to stay symmetric
//! with [`crate::envelope::PersistedEnvelope::try_new`], which rejects
//! the same value on the read path. This mirrors CLAUDE.md §3
//! ("write-path and read-path must enforce the same invariants the
//! same way") and §4 ("each crate validates at its own boundary").
//! [`decode_frame`] itself does *not* reject `schema_version == 0` —
//! its contract is faithful parsing; semantic validation is the
//! envelope's job.

use aligned_vec::{AVec, ConstAlign};
use bytes::Bytes;
use core::ops::Range;
use thiserror::Error;

/// Payload alignment in bytes. Wire-format invariant.
pub const PAYLOAD_ALIGN: usize = 16;

/// Fixed header size: `global_seq (8) + schema_version (4) + et_len (2) + meta_len (4)`.
pub const HEADER_FIXED_SIZE: usize = 18;

/// Offset of the `global_seq` field in the frame's header.
pub const GLOBAL_SEQ_OFFSET: usize = 0;

/// Offset of the `schema_version` field in the frame's header.
pub const SCHEMA_VERSION_OFFSET: usize = 8;

/// Offset of the `event_type_len` field in the frame's header.
pub const EVENT_TYPE_LEN_OFFSET: usize = 12;

/// Offset of the `meta_len` field in the frame's header.
pub const META_LEN_OFFSET: usize = 14;

/// Sentinel `meta_len` value meaning "no metadata field present".
pub const META_LEN_ABSENT: u32 = u32::MAX;

/// Maximum event-type length (matches the `u16` length-prefix field).
///
/// `u16::MAX` widens to `usize` losslessly on every supported target.
#[allow(
    clippy::as_conversions,
    reason = "const-context u16→usize widening; lossless on all targets"
)]
const MAX_EVENT_TYPE_LEN: usize = u16::MAX as usize;

/// Maximum metadata length (one less than `u32::MAX`, since `u32::MAX`
/// is the absent sentinel).
///
/// `u32::MAX` widens to `usize` losslessly on 32+ bit targets — the only
/// supported targets for nexus-store.
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
const MAX_METADATA_LEN: usize = (u32::MAX - 1) as usize;

/// Maximum payload length.
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// Bytes needed after `offset` to reach the next multiple of `align`.
///
/// Returns 0 when `offset` is already aligned. `align` must be a non-zero
/// power of two; callers pass [`PAYLOAD_ALIGN`].
#[inline]
const fn align_padding(offset: usize, align: usize) -> usize {
    (align - (offset % align)) % align
}

// ---------------------------------------------------------------------
// Length newtypes — item 5
//
// Each carries the invariant "this length fits in its wire-format field
// and (for metadata) does not collide with the absent sentinel."
// Construction via TryFrom<usize> is the only path; the inner getter
// returns the already-narrowed integer type for direct serialization.
// ---------------------------------------------------------------------

/// Event-type byte length that fits the wire `u16` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EventTypeLen(u16);

impl EventTypeLen {
    #[inline]
    const fn as_u16(self) -> u16 {
        self.0
    }
}

impl TryFrom<usize> for EventTypeLen {
    type Error = WireError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u16::try_from(value)
            .map(Self)
            .map_err(|_| WireError::EventTypeTooLong {
                actual: value,
                max: MAX_EVENT_TYPE_LEN,
            })
    }
}

/// Metadata byte length that fits the wire `u32` field and is not the
/// absent sentinel ([`META_LEN_ABSENT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetadataLen(u32);

impl MetadataLen {
    #[inline]
    const fn as_u32(self) -> u32 {
        self.0
    }
}

impl TryFrom<usize> for MetadataLen {
    type Error = WireError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value > MAX_METADATA_LEN {
            return Err(WireError::MetadataTooLong {
                actual: value,
                max: MAX_METADATA_LEN,
            });
        }
        u32::try_from(value)
            .map(Self)
            .map_err(|_| WireError::MetadataTooLong {
                actual: value,
                max: MAX_METADATA_LEN,
            })
    }
}

/// Payload byte length that fits the wire `u32` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PayloadLen(u32);

impl PayloadLen {
    #[inline]
    const fn as_u32(self) -> u32 {
        self.0
    }
}

impl TryFrom<usize> for PayloadLen {
    type Error = WireError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value)
            .map(Self)
            .map_err(|_| WireError::PayloadTooLong {
                actual: value,
                max: MAX_PAYLOAD_LEN,
            })
    }
}

// ---------------------------------------------------------------------
// FrameHeader — item 4
//
// The four fixed-position fields packed at the start of every frame.
// `write_into` serializes to exactly 18 bytes; `read_from` is its inverse.
// ---------------------------------------------------------------------

/// Fixed-position frame header (18 bytes on the wire).
///
/// Carries the four header fields together so they serialize and
/// deserialize as a unit. Use [`FrameHeader::write_into`] from the build
/// path and [`FrameHeader::read_from`] from the decode path.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub global_seq: u64,
    pub schema_version: u32,
    event_type_len: EventTypeLen,
    metadata_len: Option<MetadataLen>,
}

impl FrameHeader {
    /// Header size in bytes. Matches [`HEADER_FIXED_SIZE`].
    pub const SIZE: usize = HEADER_FIXED_SIZE;

    /// Public view of the event-type length as a `u16`.
    #[inline]
    #[must_use]
    pub const fn event_type_len(&self) -> u16 {
        self.event_type_len.as_u16()
    }

    /// Public view of the metadata length as `Option<u32>`. `None` means
    /// the absent sentinel ([`META_LEN_ABSENT`]) is on the wire.
    #[inline]
    #[must_use]
    pub const fn metadata_len(&self) -> Option<u32> {
        match self.metadata_len {
            Some(n) => Some(n.as_u32()),
            None => None,
        }
    }

    /// Serialize this header into the start of `buf` (writes exactly 18 bytes).
    fn write_into(&self, buf: &mut AVec<u8, ConstAlign<PAYLOAD_ALIGN>>) {
        let meta_field = self
            .metadata_len
            .map_or(META_LEN_ABSENT, MetadataLen::as_u32);
        buf.extend_from_slice(&self.global_seq.to_le_bytes());
        buf.extend_from_slice(&self.schema_version.to_le_bytes());
        buf.extend_from_slice(&self.event_type_len.as_u16().to_le_bytes());
        buf.extend_from_slice(&meta_field.to_le_bytes());
    }

    /// Read the fixed header from the start of `value`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::ValueTooShort`] if `value.len() < SIZE`.
    pub fn read_from(value: &[u8]) -> Result<Self, DecodeError> {
        if value.len() < Self::SIZE {
            return Err(DecodeError::ValueTooShort {
                min: Self::SIZE,
                actual: value.len(),
            });
        }
        let global_seq = u64::from_le_bytes([
            value[GLOBAL_SEQ_OFFSET],
            value[GLOBAL_SEQ_OFFSET + 1],
            value[GLOBAL_SEQ_OFFSET + 2],
            value[GLOBAL_SEQ_OFFSET + 3],
            value[GLOBAL_SEQ_OFFSET + 4],
            value[GLOBAL_SEQ_OFFSET + 5],
            value[GLOBAL_SEQ_OFFSET + 6],
            value[GLOBAL_SEQ_OFFSET + 7],
        ]);
        let schema_version = u32::from_le_bytes([
            value[SCHEMA_VERSION_OFFSET],
            value[SCHEMA_VERSION_OFFSET + 1],
            value[SCHEMA_VERSION_OFFSET + 2],
            value[SCHEMA_VERSION_OFFSET + 3],
        ]);
        let event_type_len = EventTypeLen(u16::from_le_bytes([
            value[EVENT_TYPE_LEN_OFFSET],
            value[EVENT_TYPE_LEN_OFFSET + 1],
        ]));
        let meta_field = u32::from_le_bytes([
            value[META_LEN_OFFSET],
            value[META_LEN_OFFSET + 1],
            value[META_LEN_OFFSET + 2],
            value[META_LEN_OFFSET + 3],
        ]);
        let metadata_len = if meta_field == META_LEN_ABSENT {
            None
        } else {
            Some(MetadataLen(meta_field))
        };
        Ok(Self {
            global_seq,
            schema_version,
            event_type_len,
            metadata_len,
        })
    }
}

// ---------------------------------------------------------------------
// FrameLayout — pure arithmetic (no buffer touches)
// ---------------------------------------------------------------------

/// Byte layout of one frame: where each field lives and how big the buffer is.
///
/// Produced by [`FrameLayout::compute`] from already-validated length
/// newtypes; the build path uses every field, the decode path uses only
/// the padding formula via [`align_padding`].
#[derive(Debug, Clone)]
struct FrameLayout {
    event_type: Range<u32>,
    metadata: Option<Range<u32>>,
    payload: Range<u32>,
    padding: usize,
    total: usize,
}

/// Build a [`WireError::FrameLengthOverflow`] from its three diagnostic fields.
#[inline]
const fn length_overflow(header: usize, padding: usize, payload: usize) -> WireError {
    WireError::FrameLengthOverflow {
        header,
        padding,
        payload,
    }
}

impl FrameLayout {
    /// Compute the layout from validated lengths.
    ///
    /// # Errors
    ///
    /// Returns [`WireError::FrameLengthOverflow`] if combining the
    /// fields would overflow `usize` on the target platform or any
    /// computed offset would not fit in `u32`.
    fn compute(
        event_type_len: EventTypeLen,
        metadata_len: Option<MetadataLen>,
        payload_len: PayloadLen,
    ) -> Result<Self, WireError> {
        let et_len = usize::from(event_type_len.as_u16());
        let meta_len_usize = metadata_len
            .map(|n| {
                usize::try_from(n.as_u32()).map_err(|_| length_overflow(HEADER_FIXED_SIZE, 0, 0))
            })
            .transpose()?
            .unwrap_or(0);
        let pay_len = usize::try_from(payload_len.as_u32())
            .map_err(|_| length_overflow(HEADER_FIXED_SIZE, 0, 0))?;

        let pre_payload_len = HEADER_FIXED_SIZE
            .checked_add(et_len)
            .and_then(|n| n.checked_add(meta_len_usize))
            .ok_or_else(|| length_overflow(HEADER_FIXED_SIZE, 0, pay_len))?;

        let padding = align_padding(pre_payload_len, PAYLOAD_ALIGN);
        let total = pre_payload_len
            .checked_add(padding)
            .and_then(|n| n.checked_add(pay_len))
            .ok_or_else(|| length_overflow(pre_payload_len, padding, pay_len))?;

        let overflow = || length_overflow(pre_payload_len, padding, pay_len);

        let event_type_start = u32::try_from(HEADER_FIXED_SIZE).map_err(|_| overflow())?;
        let event_type_end = event_type_start
            .checked_add(u32::from(event_type_len.as_u16()))
            .ok_or_else(overflow)?;

        let metadata_range = metadata_len
            .map(|n| -> Result<Range<u32>, WireError> {
                let end = event_type_end
                    .checked_add(n.as_u32())
                    .ok_or_else(overflow)?;
                Ok(event_type_end..end)
            })
            .transpose()?;

        let payload_start_usize = pre_payload_len.checked_add(padding).ok_or_else(overflow)?;
        let payload_start = u32::try_from(payload_start_usize).map_err(|_| overflow())?;
        let payload_end = payload_start
            .checked_add(payload_len.as_u32())
            .ok_or_else(overflow)?;

        Ok(Self {
            event_type: event_type_start..event_type_end,
            metadata: metadata_range,
            payload: payload_start..payload_end,
            padding,
            total,
        })
    }
}

// ---------------------------------------------------------------------
// Public output / error types
// ---------------------------------------------------------------------

/// Output of [`encode_frame`]: the assembled buffer plus byte ranges into it.
#[derive(Debug)]
pub struct EncodedFrame {
    pub value: Bytes,
    pub offsets: FrameOffsets,
}

/// Byte ranges for each variable-width field within an [`EncodedFrame::value`] buffer.
///
/// Fixed-position header fields (`global_seq`, `schema_version`,
/// `event_type_len`, `meta_len`) are read from constant offsets and
/// have no ranges here.
#[derive(Debug, Clone)]
pub struct FrameOffsets {
    pub event_type: Range<u32>,
    pub metadata: Option<Range<u32>>,
    pub payload: Range<u32>,
}

/// Errors from [`encode_frame`].
#[derive(Debug, Error)]
pub enum WireError {
    /// `schema_version == 0` is rejected. Mirrors the read-path check in
    /// [`crate::envelope::PersistedEnvelope::try_new`] so the two paths
    /// enforce the same invariant — see CLAUDE.md §4 "Each crate
    /// validates at its own boundary."
    #[error("schema_version must be > 0 (got 0)")]
    InvalidSchemaVersion,
    #[error("event type length {actual} exceeds maximum {max}")]
    EventTypeTooLong { actual: usize, max: usize },
    #[error("metadata length {actual} exceeds maximum {max}")]
    MetadataTooLong { actual: usize, max: usize },
    #[error("payload length {actual} exceeds maximum {max}")]
    PayloadTooLong { actual: usize, max: usize },
    #[error(
        "frame length overflow combining header={header}, padding={padding}, payload={payload}"
    )]
    FrameLengthOverflow {
        header: usize,
        padding: usize,
        payload: usize,
    },
}

/// Output of [`decode_frame`]: header fields plus byte ranges into the input value.
#[derive(Debug)]
pub struct DecodedFrame {
    pub global_seq: u64,
    pub schema_version: u32,
    pub offsets: FrameOffsets,
}

/// Errors from [`decode_frame`].
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },
    #[error("event type length {et_len} extends past value (len={value_len})")]
    EventTypeTruncated { et_len: usize, value_len: usize },
    #[error("metadata length {meta_len} extends past value (len={value_len})")]
    MetadataTruncated { meta_len: u32, value_len: usize },
    #[error("computed offset overflows u32 (value len={value_len})")]
    OffsetOverflow { value_len: usize },
}

// ---------------------------------------------------------------------
// FramePlan + plan/execute — item 6
//
// `plan` does all fallible work (validation + layout math).
// `execute` is infallible — given a plan, fill the buffer.
// ---------------------------------------------------------------------

/// Everything needed to materialize one frame's bytes.
///
/// Construction via [`plan`] guarantees: every length already fits its
/// wire field, the layout has been computed without overflow, and the
/// borrowed slices are the body bytes the layout describes.
#[derive(Debug)]
struct FramePlan<'a> {
    header: FrameHeader,
    event_type_bytes: &'a [u8],
    metadata: Option<&'a [u8]>,
    payload: &'a [u8],
    layout: FrameLayout,
}

/// Validate inputs and compute the layout for one frame.
fn plan<'a>(
    global_seq: u64,
    schema_version: u32,
    event_type: &'a str,
    metadata: Option<&'a [u8]>,
    payload: &'a [u8],
) -> Result<FramePlan<'a>, WireError> {
    if schema_version == 0 {
        return Err(WireError::InvalidSchemaVersion);
    }
    let event_type_bytes = event_type.as_bytes();
    let event_type_len = EventTypeLen::try_from(event_type_bytes.len())?;
    let metadata_len = metadata
        .map(|m| MetadataLen::try_from(m.len()))
        .transpose()?;
    let payload_len = PayloadLen::try_from(payload.len())?;

    let layout = FrameLayout::compute(event_type_len, metadata_len, payload_len)?;
    let header = FrameHeader {
        global_seq,
        schema_version,
        event_type_len,
        metadata_len,
    };
    Ok(FramePlan {
        header,
        event_type_bytes,
        metadata,
        payload,
        layout,
    })
}

/// Materialize a plan into an aligned buffer. Infallible.
fn execute(plan: FramePlan<'_>) -> EncodedFrame {
    let mut buf: AVec<u8, ConstAlign<PAYLOAD_ALIGN>> =
        AVec::with_capacity(PAYLOAD_ALIGN, plan.layout.total);
    plan.header.write_into(&mut buf);
    buf.extend_from_slice(plan.event_type_bytes);
    if let Some(m) = plan.metadata {
        buf.extend_from_slice(m);
    }
    buf.resize(buf.len() + plan.layout.padding, 0u8);
    buf.extend_from_slice(plan.payload);

    EncodedFrame {
        value: Bytes::from_owner(buf),
        offsets: FrameOffsets {
            event_type: plan.layout.event_type,
            metadata: plan.layout.metadata,
            payload: plan.layout.payload,
        },
    }
}

// ---------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------

/// Build one frame buffer with payload aligned to [`PAYLOAD_ALIGN`].
///
/// Layout:
///
/// ```text
/// [u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len]
/// [u32 LE meta_len][event_type bytes][metadata bytes if any]
/// [padding zero-bytes][payload bytes]
/// ```
///
/// `meta_len == u32::MAX` is the absent-metadata sentinel.
///
/// # Errors
///
/// Returns [`WireError`] if any field exceeds its maximum length or if
/// the assembled frame would overflow `usize` on the target platform.
pub fn encode_frame(
    global_seq: u64,
    schema_version: u32,
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<EncodedFrame, WireError> {
    Ok(execute(plan(
        global_seq,
        schema_version,
        event_type,
        metadata,
        payload,
    )?))
}

/// Decode a frame value built by [`encode_frame`].
///
/// Reads the fixed header, recovers event-type and metadata ranges, and
/// computes the payload range honoring the 16-byte alignment padding.
///
/// # Errors
///
/// - [`DecodeError::ValueTooShort`] if `value` is shorter than the fixed header.
/// - [`DecodeError::EventTypeTruncated`] if the event-type length runs past the buffer.
/// - [`DecodeError::MetadataTruncated`] if `meta_len` claims bytes past the buffer end.
/// - [`DecodeError::OffsetOverflow`] if any computed offset would not fit in `u32`.
pub fn decode_frame(value: &[u8]) -> Result<DecodedFrame, DecodeError> {
    let header = FrameHeader::read_from(value)?;
    let et_len = usize::from(header.event_type_len.as_u16());

    let et_start = HEADER_FIXED_SIZE;
    let et_end = et_start
        .checked_add(et_len)
        .ok_or(DecodeError::OffsetOverflow {
            value_len: value.len(),
        })?;
    if value.len() < et_end {
        return Err(DecodeError::EventTypeTruncated {
            et_len,
            value_len: value.len(),
        });
    }

    let (metadata_range, post_meta) = match header.metadata_len {
        None => (None, et_end),
        Some(meta_len) => {
            let meta_len_usize =
                usize::try_from(meta_len.as_u32()).map_err(|_| DecodeError::OffsetOverflow {
                    value_len: value.len(),
                })?;
            let meta_end =
                et_end
                    .checked_add(meta_len_usize)
                    .ok_or(DecodeError::OffsetOverflow {
                        value_len: value.len(),
                    })?;
            if value.len() < meta_end {
                return Err(DecodeError::MetadataTruncated {
                    meta_len: meta_len.as_u32(),
                    value_len: value.len(),
                });
            }
            let m_start_u32 = u32::try_from(et_end).map_err(|_| DecodeError::OffsetOverflow {
                value_len: value.len(),
            })?;
            let m_end_u32 = u32::try_from(meta_end).map_err(|_| DecodeError::OffsetOverflow {
                value_len: value.len(),
            })?;
            (Some(m_start_u32..m_end_u32), meta_end)
        }
    };

    let padding = align_padding(post_meta, PAYLOAD_ALIGN);
    let payload_start = post_meta
        .checked_add(padding)
        .ok_or(DecodeError::OffsetOverflow {
            value_len: value.len(),
        })?;
    let payload_end = value.len();
    if payload_start > payload_end {
        return Err(DecodeError::OffsetOverflow {
            value_len: value.len(),
        });
    }

    let et_start_u32 = u32::try_from(et_start).map_err(|_| DecodeError::OffsetOverflow {
        value_len: value.len(),
    })?;
    let et_end_u32 = u32::try_from(et_end).map_err(|_| DecodeError::OffsetOverflow {
        value_len: value.len(),
    })?;
    let payload_start_u32 =
        u32::try_from(payload_start).map_err(|_| DecodeError::OffsetOverflow {
            value_len: value.len(),
        })?;
    let payload_end_u32 = u32::try_from(payload_end).map_err(|_| DecodeError::OffsetOverflow {
        value_len: value.len(),
    })?;

    Ok(DecodedFrame {
        global_seq: header.global_seq,
        schema_version: header.schema_version,
        offsets: FrameOffsets {
            event_type: et_start_u32..et_end_u32,
            metadata: metadata_range,
            payload: payload_start_u32..payload_end_u32,
        },
    })
}

#[cfg(test)]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::panic,
    clippy::redundant_clone,
    clippy::single_match_else,
    reason = "test code: index arithmetic, prop_assert_eq macro expansions, \
              and `panic!(\"expected X, got {other:?}\")` arms surface failing test diagnostics"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn payload_ptr_aligned(frame: &EncodedFrame) -> bool {
        let start = usize::try_from(frame.offsets.payload.start).expect("u32 fits usize");
        let end = usize::try_from(frame.offsets.payload.end).expect("u32 fits usize");
        let payload_slice = &frame.value[start..end];
        payload_slice.as_ptr().addr().is_multiple_of(PAYLOAD_ALIGN)
    }

    // -----------------------------------------------------------------
    // Reusable strategy helpers
    //
    // Each follows the project's "include 0, 1, MAX-1, MAX via prop_oneof!
    // alongside the interior range" rule. Weights are 1 per boundary and
    // 10 for the interior — boundaries are still always hit (~28% of
    // runs collectively) without choking interior coverage.
    // -----------------------------------------------------------------

    /// Couples align (power of 2) with a boundary-rich offset for that
    /// align. Generated jointly via `prop_flat_map` so shrinking can
    /// narrow to the minimum `(align, offset)` pair that violates an
    /// invariant — see the book's "Higher-Order Strategies" chapter.
    fn align_and_offset() -> impl Strategy<Value = (usize, usize)> {
        (0u32..16).prop_flat_map(|align_pow| {
            let align = 1usize << align_pow;
            // align - 1 may equal 0 when align == 1; duplicates Just(0).
            let offset = prop_oneof![
                1 => Just(0usize),
                1 => Just(1usize),
                1 => Just(align.saturating_sub(1)),
                1 => Just(align),
                1 => Just(align + 1),
                10 => 0usize..1_000_000,
            ];
            (Just(align), offset)
        })
    }

    fn u64_strategy() -> impl Strategy<Value = u64> {
        prop_oneof![
            1 => Just(0u64),
            1 => Just(1u64),
            1 => Just(u64::MAX - 1),
            1 => Just(u64::MAX),
            10 => any::<u64>(),
        ]
    }

    fn u32_strategy() -> impl Strategy<Value = u32> {
        prop_oneof![
            1 => Just(0u32),
            1 => Just(1u32),
            1 => Just(u32::MAX - 1),
            1 => Just(u32::MAX),
            10 => any::<u32>(),
        ]
    }

    /// Nonzero `schema_version` strategy. `encode_frame` rejects 0 to
    /// stay symmetric with [`crate::envelope::PersistedEnvelope::try_new`],
    /// so the `encode_frame`-driving composite below must never generate
    /// it. Boundaries follow the project's `0/1/MAX-1/MAX via prop_oneof!`
    /// rule, adjusted for the nonzero domain.
    fn schema_version_strategy() -> impl Strategy<Value = u32> {
        prop_oneof![
            1 => Just(1u32),
            1 => Just(2u32),
            1 => Just(u32::MAX - 1),
            1 => Just(u32::MAX),
            10 => 1u32..=u32::MAX,
        ]
    }

    fn u16_strategy() -> impl Strategy<Value = u16> {
        prop_oneof![
            1 => Just(0u16),
            1 => Just(1u16),
            1 => Just(u16::MAX - 1),
            1 => Just(u16::MAX),
            10 => any::<u16>(),
        ]
    }

    fn event_type_len_strategy() -> impl Strategy<Value = usize> {
        prop_oneof![
            1 => Just(0usize),
            1 => Just(1usize),
            1 => Just(MAX_EVENT_TYPE_LEN - 1),
            1 => Just(MAX_EVENT_TYPE_LEN),
            10 => 0usize..=MAX_EVENT_TYPE_LEN,
        ]
    }

    fn metadata_len_strategy() -> impl Strategy<Value = usize> {
        prop_oneof![
            1 => Just(0usize),
            1 => Just(1usize),
            1 => Just(MAX_METADATA_LEN - 1),
            1 => Just(MAX_METADATA_LEN),
            10 => 0usize..=MAX_METADATA_LEN,
        ]
    }

    fn payload_len_strategy() -> impl Strategy<Value = usize> {
        prop_oneof![
            1 => Just(0usize),
            1 => Just(1usize),
            1 => Just(MAX_PAYLOAD_LEN - 1),
            1 => Just(MAX_PAYLOAD_LEN),
            10 => 0usize..=MAX_PAYLOAD_LEN,
        ]
    }

    /// Bounded length strategy for layout-time tests where the input is
    /// also a `Vec<u8>` allocation; capped low to keep tests fast while
    /// preserving the boundary cases that matter for layout arithmetic.
    fn frame_body_length() -> impl Strategy<Value = usize> {
        prop_oneof![
            1 => Just(0usize),
            1 => Just(1usize),
            1 => Just(PAYLOAD_ALIGN - 1),
            1 => Just(PAYLOAD_ALIGN),
            1 => Just(PAYLOAD_ALIGN + 1),
            10 => 0usize..=4096,
        ]
    }

    /// UTF-8 event-type strings with explicit boundary anchors plus a
    /// Unicode-complete interior. `any::<char>()` covers the full code
    /// point space, which the wire format accepts (the only constraint
    /// is byte length under [`MAX_EVENT_TYPE_LEN`]).
    fn event_type_str_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            1 => Just(String::new()),
            1 => Just("a".to_owned()),
            10 => prop::collection::vec(any::<char>(), 0..=256)
                .prop_map(|chars| chars.into_iter().collect::<String>()),
        ]
    }

    fn metadata_bytes_strategy() -> impl Strategy<Value = Option<Vec<u8>>> {
        prop_oneof![
            1 => Just(None),
            1 => Just(Some(Vec::<u8>::new())),
            1 => Just(Some(vec![0u8])),
            10 => prop::option::of(prop::collection::vec(any::<u8>(), 0..512)),
        ]
    }

    fn payload_bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            1 => Just(Vec::<u8>::new()),
            1 => Just(vec![0u8]),
            10 => prop::collection::vec(any::<u8>(), 0..2048),
        ]
    }

    // Composite: every input to `encode_frame` joined into one strategy
    // via `prop_compose!` (the book's pattern for named composites).
    // Shrinking remains coordinated across the five components.
    prop_compose! {
        fn valid_frame_inputs()(
            global_seq in u64_strategy(),
            schema_version in schema_version_strategy(),
            event_type in event_type_str_strategy(),
            metadata in metadata_bytes_strategy(),
            payload in payload_bytes_strategy(),
        ) -> (u64, u32, String, Option<Vec<u8>>, Vec<u8>) {
            (global_seq, schema_version, event_type, metadata, payload)
        }
    }

    proptest! {
        #[test]
        fn payload_pointer_is_16_aligned(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let frame = encode_frame(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("encode_frame succeeds on bounded inputs");
            prop_assert!(payload_ptr_aligned(&frame));
        }

        #[test]
        fn ranges_recover_each_field(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let frame = encode_frame(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("encode_frame succeeds on bounded inputs");
            let v = &frame.value;
            prop_assert_eq!(
                &v[frame.offsets.event_type.start as usize..frame.offsets.event_type.end as usize],
                event_type.as_bytes()
            );
            prop_assert_eq!(
                &v[frame.offsets.payload.start as usize..frame.offsets.payload.end as usize],
                payload.as_slice()
            );
            if let (Some(meta), Some(range)) = (meta_ref, frame.offsets.metadata) {
                prop_assert_eq!(
                    &v[range.start as usize..range.end as usize],
                    meta
                );
            }
        }

        #[test]
        fn header_fields_are_recoverable(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let frame = encode_frame(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("encode_frame succeeds on bounded inputs");
            let v = &frame.value;

            let mut gs_buf = [0u8; 8];
            gs_buf.copy_from_slice(&v[GLOBAL_SEQ_OFFSET..GLOBAL_SEQ_OFFSET + 8]);
            prop_assert_eq!(u64::from_le_bytes(gs_buf), global_seq);

            let mut sv_buf = [0u8; 4];
            sv_buf.copy_from_slice(&v[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4]);
            prop_assert_eq!(u32::from_le_bytes(sv_buf), schema_version);

            let mut et_len_buf = [0u8; 2];
            et_len_buf.copy_from_slice(&v[EVENT_TYPE_LEN_OFFSET..EVENT_TYPE_LEN_OFFSET + 2]);
            prop_assert_eq!(usize::from(u16::from_le_bytes(et_len_buf)), event_type.len());

            let mut ml_buf = [0u8; 4];
            ml_buf.copy_from_slice(&v[META_LEN_OFFSET..META_LEN_OFFSET + 4]);
            let ml = u32::from_le_bytes(ml_buf);
            match meta_ref {
                Some(m) => prop_assert_eq!(usize::try_from(ml).unwrap(), m.len()),
                None => prop_assert_eq!(ml, META_LEN_ABSENT),
            }
        }
    }

    #[test]
    fn empty_payload_still_aligned() {
        let frame = encode_frame(1, 1, "X", None, &[]).expect("trivial frame builds");
        assert!(payload_ptr_aligned(&frame));
        assert_eq!(frame.offsets.payload.start, frame.offsets.payload.end);
    }

    #[test]
    fn empty_event_type_permitted() {
        let frame =
            encode_frame(1, 1, "", None, b"data").expect("empty event_type accepted at wire layer");
        assert!(payload_ptr_aligned(&frame));
    }

    #[test]
    fn max_event_type_accepted() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN);
        encode_frame(1, 1, &huge, None, b"d").expect("max-length event_type accepted");
    }

    #[test]
    fn over_max_event_type_rejected() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN + 1);
        assert!(matches!(
            encode_frame(1, 1, &huge, None, b"d"),
            Err(WireError::EventTypeTooLong { .. })
        ));
    }

    #[test]
    fn meta_len_u32_max_is_absent_sentinel() {
        let frame = encode_frame(1, 1, "X", None, b"d").expect("none-metadata frame builds");
        let mut ml_buf = [0u8; 4];
        ml_buf.copy_from_slice(&frame.value[META_LEN_OFFSET..META_LEN_OFFSET + 4]);
        assert_eq!(u32::from_le_bytes(ml_buf), META_LEN_ABSENT);
        assert!(frame.offsets.metadata.is_none());
    }

    proptest! {
        #[test]
        fn build_then_decode_round_trips(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let frame = encode_frame(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("encode_frame succeeds on bounded inputs");
            let decoded = decode_frame(&frame.value).expect("decode_frame succeeds on a built frame");
            prop_assert_eq!(decoded.global_seq, global_seq);
            prop_assert_eq!(decoded.schema_version, schema_version);
            prop_assert_eq!(decoded.offsets.event_type.clone(), frame.offsets.event_type.clone());
            prop_assert_eq!(decoded.offsets.metadata.clone(), frame.offsets.metadata.clone());
            prop_assert_eq!(decoded.offsets.payload.clone(), frame.offsets.payload.clone());
        }
    }

    #[test]
    fn decode_rejects_truncated_value() {
        let too_short = vec![0u8; HEADER_FIXED_SIZE - 1];
        assert!(matches!(
            decode_frame(&too_short),
            Err(DecodeError::ValueTooShort { .. })
        ));
    }

    #[test]
    fn decode_rejects_truncated_event_type() {
        // Header claims et_len = 100 but no event-type bytes follow.
        let mut buf = vec![0u8; HEADER_FIXED_SIZE];
        buf[EVENT_TYPE_LEN_OFFSET..EVENT_TYPE_LEN_OFFSET + 2]
            .copy_from_slice(&100u16.to_le_bytes());
        buf[META_LEN_OFFSET..META_LEN_OFFSET + 4].copy_from_slice(&META_LEN_ABSENT.to_le_bytes());
        assert!(matches!(
            decode_frame(&buf),
            Err(DecodeError::EventTypeTruncated { .. })
        ));
    }

    #[test]
    fn decode_rejects_truncated_metadata() {
        // Header claims meta_len = 100 but no metadata bytes follow.
        let mut buf = vec![0u8; HEADER_FIXED_SIZE];
        buf[EVENT_TYPE_LEN_OFFSET..EVENT_TYPE_LEN_OFFSET + 2].copy_from_slice(&0u16.to_le_bytes());
        buf[META_LEN_OFFSET..META_LEN_OFFSET + 4].copy_from_slice(&100u32.to_le_bytes());
        assert!(matches!(
            decode_frame(&buf),
            Err(DecodeError::MetadataTruncated { .. })
        ));
    }

    // -----------------------------------------------------------------
    // schema_version asymmetry repair — both the build path and
    // PersistedEnvelope::try_new must reject 0. CLAUDE.md §4 ("each
    // crate validates at its own boundary") plus §3 ("write-path and
    // read-path must enforce the same invariants the same way").
    // -----------------------------------------------------------------

    #[test]
    fn encode_frame_rejects_schema_version_zero() {
        match encode_frame(1, 0, "Evt", None, b"data") {
            Err(WireError::InvalidSchemaVersion) => {}
            other => panic!("expected InvalidSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn plan_rejects_schema_version_zero() {
        // plan() is the validation stage — surfacing the variant here
        // pins the check at the right layer (not in execute()).
        match plan(1, 0, "Evt", None, b"data") {
            Err(WireError::InvalidSchemaVersion) => {}
            other => panic!("expected InvalidSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn encode_frame_accepts_schema_version_one() {
        // The minimum valid schema_version. Boundary case for the
        // newly-introduced rejection — if the off-by-one is wrong,
        // this test fires.
        encode_frame(1, 1, "Evt", None, b"data").expect("schema_version=1 is valid");
    }

    #[test]
    fn encode_frame_accepts_schema_version_u32_max() {
        // Upper boundary — encode_frame should accept the entire
        // nonzero u32 range, not artificially cap below MAX.
        encode_frame(1, u32::MAX, "Evt", None, b"data").expect("schema_version=u32::MAX is valid");
    }

    // -----------------------------------------------------------------
    // decode_frame panic-freedom — adversarial input
    //
    // Wire-format frames stored at rest may be corrupt (disk bit-rot,
    // truncation, malicious tampering). decode_frame must surface every
    // failure as a typed DecodeError; a panic would crash the host
    // process on a single bad row. proptest catches panics as failures,
    // so the absence of `prop_assert!`/`assert!` in the body is
    // intentional — the test asserts "did not panic" by surviving.
    // -----------------------------------------------------------------

    /// Buffers shaped to exercise every branch of [`decode_frame`].
    ///
    /// - empty / 1-byte: trigger the `ValueTooShort` early-return.
    /// - lengths around `HEADER_FIXED_SIZE`: pin the exact threshold.
    /// - raw random bytes: most random headers claim huge `et_len`, so
    ///   they exercise the `EventTypeTruncated` path well.
    /// - header-shaped: bound `et_len`/`meta_len` to plausible values
    ///   so random bodies reach the metadata- and payload-range arms
    ///   that raw random would skip ~94% of the time.
    fn adversarial_decode_bytes() -> impl Strategy<Value = Vec<u8>> {
        // Header-shaped: small et_len / meta_len, random body. Drives
        // the deeper code paths that raw random rarely reaches.
        let header_shaped = (
            any::<u64>(),
            any::<u32>(),
            0u16..=64,
            prop_oneof![Just(META_LEN_ABSENT), 0u32..=64],
            prop::collection::vec(any::<u8>(), 0..=512),
        )
            .prop_map(|(gs, sv, et_len, meta_len, body)| {
                let mut buf = Vec::with_capacity(HEADER_FIXED_SIZE + body.len());
                buf.extend_from_slice(&gs.to_le_bytes());
                buf.extend_from_slice(&sv.to_le_bytes());
                buf.extend_from_slice(&et_len.to_le_bytes());
                buf.extend_from_slice(&meta_len.to_le_bytes());
                buf.extend_from_slice(&body);
                buf
            });

        prop_oneof![
            1 => Just(Vec::<u8>::new()),
            1 => Just(vec![0u8]),
            1 => prop::collection::vec(any::<u8>(), HEADER_FIXED_SIZE - 1..=HEADER_FIXED_SIZE - 1),
            1 => prop::collection::vec(any::<u8>(), HEADER_FIXED_SIZE..=HEADER_FIXED_SIZE),
            1 => prop::collection::vec(any::<u8>(), HEADER_FIXED_SIZE + 1..=HEADER_FIXED_SIZE + 1),
            5 => prop::collection::vec(any::<u8>(), 0..=4096),
            5 => header_shaped,
        ]
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in adversarial_decode_bytes()) {
            // The assertion is structural: proptest treats panics as
            // failures, so reaching the end of the closure with any
            // Result is a pass. Every fallible step in decode_frame is
            // a `?` to a typed DecodeError variant — this test pins
            // that claim end-to-end on arbitrary input.
            let _ = decode_frame(&bytes);
        }

        /// Stronger claim: when `decode_frame` succeeds on adversarial
        /// input, the returned ranges must be in-bounds. A bug that
        /// returns out-of-range offsets is just as dangerous as a panic
        /// — the next slice index by a consumer would panic instead.
        #[test]
        fn decode_offsets_in_bounds_on_success(bytes in adversarial_decode_bytes()) {
            if let Ok(decoded) = decode_frame(&bytes) {
                let len_u32 = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                prop_assert!(decoded.offsets.event_type.start <= decoded.offsets.event_type.end);
                prop_assert!(decoded.offsets.event_type.end <= len_u32);
                if let Some(meta) = decoded.offsets.metadata {
                    prop_assert!(meta.start <= meta.end);
                    prop_assert!(meta.end <= len_u32);
                }
                prop_assert!(decoded.offsets.payload.start <= decoded.offsets.payload.end);
                prop_assert!(decoded.offsets.payload.end <= len_u32);
            }
        }
    }

    // -----------------------------------------------------------------
    // align_padding
    // -----------------------------------------------------------------

    #[test]
    fn align_padding_zero_offset_yields_zero() {
        assert_eq!(align_padding(0, PAYLOAD_ALIGN), 0);
    }

    #[test]
    fn align_padding_one_below_boundary_yields_one() {
        assert_eq!(align_padding(15, PAYLOAD_ALIGN), 1);
    }

    #[test]
    fn align_padding_on_boundary_yields_zero() {
        assert_eq!(align_padding(PAYLOAD_ALIGN, PAYLOAD_ALIGN), 0);
    }

    #[test]
    fn align_padding_one_above_boundary_yields_fifteen() {
        assert_eq!(align_padding(PAYLOAD_ALIGN + 1, PAYLOAD_ALIGN), 15);
    }

    proptest! {
        // `align_and_offset()` generates (align, offset) jointly via
        // `prop_flat_map`, so when an invariant breaks proptest shrinks
        // to the minimum failing pair — not just a seed value the loop
        // happened to derive.
        #[test]
        fn align_padding_invariants(
            (align, offset) in align_and_offset(),
        ) {
            let pad = align_padding(offset, align);

            // Invariant I1: offset + pad is a multiple of align.
            prop_assert!(
                (offset + pad).is_multiple_of(align),
                "offset={offset} align={align} pad={pad} not multiple",
            );

            // Invariant I2: pad < align — the function returns the
            // *minimum* padding to reach the next multiple, never more.
            prop_assert!(pad < align, "pad {pad} >= align {align}");

            // Invariant I3: pad == 0 iff offset is already aligned.
            prop_assert_eq!(pad == 0, offset.is_multiple_of(align));
        }
    }

    // -----------------------------------------------------------------
    // Length newtypes — boundary-rich coverage of every invariant
    // -----------------------------------------------------------------

    // EventTypeLen — fits u16 (max 65535).

    #[test]
    fn event_type_len_accepts_zero() {
        let v = EventTypeLen::try_from(0usize).expect("0 accepted");
        assert_eq!(v.as_u16(), 0);
    }

    #[test]
    fn event_type_len_accepts_one() {
        assert_eq!(EventTypeLen::try_from(1usize).unwrap().as_u16(), 1);
    }

    #[test]
    fn event_type_len_accepts_max_minus_one() {
        assert_eq!(
            EventTypeLen::try_from(MAX_EVENT_TYPE_LEN - 1)
                .unwrap()
                .as_u16(),
            u16::MAX - 1,
        );
    }

    #[test]
    fn event_type_len_accepts_max() {
        assert_eq!(
            EventTypeLen::try_from(MAX_EVENT_TYPE_LEN).unwrap().as_u16(),
            u16::MAX,
        );
    }

    #[test]
    fn event_type_len_rejects_max_plus_one() {
        // The error must carry the *actual* offending value, not zero.
        match EventTypeLen::try_from(MAX_EVENT_TYPE_LEN + 1) {
            Err(WireError::EventTypeTooLong { actual, max }) => {
                assert_eq!(actual, MAX_EVENT_TYPE_LEN + 1);
                assert_eq!(max, MAX_EVENT_TYPE_LEN);
            }
            other => panic!("expected EventTypeTooLong, got {other:?}"),
        }
    }

    proptest! {
        #[test]
        fn event_type_len_round_trip(len in event_type_len_strategy()) {
            let v = EventTypeLen::try_from(len).expect("valid length");
            prop_assert_eq!(usize::from(v.as_u16()), len);
        }
    }

    // MetadataLen — fits u32 but u32::MAX is reserved as the absent sentinel.

    #[test]
    fn metadata_len_accepts_zero() {
        // Some(0) — empty metadata, distinct from None.
        assert_eq!(MetadataLen::try_from(0usize).unwrap().as_u32(), 0);
    }

    #[test]
    fn metadata_len_accepts_max_minus_one() {
        // MAX_METADATA_LEN - 1 = u32::MAX - 2, well under sentinel.
        assert_eq!(
            MetadataLen::try_from(MAX_METADATA_LEN - 1)
                .unwrap()
                .as_u32(),
            u32::MAX - 2,
        );
    }

    #[test]
    fn metadata_len_accepts_max() {
        // MAX_METADATA_LEN = u32::MAX - 1, the largest value not colliding with sentinel.
        assert_eq!(
            MetadataLen::try_from(MAX_METADATA_LEN).unwrap().as_u32(),
            u32::MAX - 1,
        );
    }

    #[test]
    fn metadata_len_rejects_at_sentinel_value() {
        // u32::MAX itself = MAX_METADATA_LEN + 1 — reserved as absent sentinel.
        match MetadataLen::try_from(MAX_METADATA_LEN + 1) {
            Err(WireError::MetadataTooLong { actual, max }) => {
                assert_eq!(actual, MAX_METADATA_LEN + 1);
                assert_eq!(max, MAX_METADATA_LEN);
            }
            other => panic!("expected MetadataTooLong, got {other:?}"),
        }
    }

    proptest! {
        #[test]
        fn metadata_len_round_trip(len in metadata_len_strategy()) {
            let v = MetadataLen::try_from(len).expect("valid length");
            prop_assert_eq!(usize::try_from(v.as_u32()).unwrap(), len);
        }
    }

    // PayloadLen — fits u32.

    #[test]
    fn payload_len_accepts_zero() {
        assert_eq!(PayloadLen::try_from(0usize).unwrap().as_u32(), 0);
    }

    #[test]
    fn payload_len_accepts_max() {
        assert_eq!(
            PayloadLen::try_from(MAX_PAYLOAD_LEN).unwrap().as_u32(),
            u32::MAX,
        );
    }

    proptest! {
        #[test]
        fn payload_len_round_trip(len in payload_len_strategy()) {
            let v = PayloadLen::try_from(len).expect("valid length");
            prop_assert_eq!(usize::try_from(v.as_u32()).unwrap(), len);
        }
    }

    // -----------------------------------------------------------------
    // FrameHeader
    // -----------------------------------------------------------------

    fn fresh_buf() -> AVec<u8, ConstAlign<PAYLOAD_ALIGN>> {
        AVec::with_capacity(PAYLOAD_ALIGN, 64)
    }

    #[test]
    fn frame_header_write_into_writes_all_fields_at_correct_offsets() {
        // Distinct byte patterns per field so a mis-offset would show up.
        let header = FrameHeader {
            global_seq: 0x0102_0304_0506_0708,
            schema_version: 0x090A_0B0C,
            event_type_len: EventTypeLen(0x0D0E),
            metadata_len: Some(MetadataLen(0x0F10_1112)),
        };
        let mut buf = fresh_buf();
        header.write_into(&mut buf);

        // Invariant: writes exactly SIZE bytes.
        assert_eq!(buf.len(), FrameHeader::SIZE);

        // Invariant: every field lives at its declared constant offset
        // in little-endian. Asserting all four catches mis-offset bugs
        // a spot check would miss.
        assert_eq!(
            &buf[GLOBAL_SEQ_OFFSET..GLOBAL_SEQ_OFFSET + 8],
            &0x0102_0304_0506_0708u64.to_le_bytes(),
        );
        assert_eq!(
            &buf[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4],
            &0x090A_0B0Cu32.to_le_bytes(),
        );
        assert_eq!(
            &buf[EVENT_TYPE_LEN_OFFSET..EVENT_TYPE_LEN_OFFSET + 2],
            &0x0D0Eu16.to_le_bytes(),
        );
        assert_eq!(
            &buf[META_LEN_OFFSET..META_LEN_OFFSET + 4],
            &0x0F10_1112u32.to_le_bytes(),
        );
    }

    #[test]
    fn frame_header_none_metadata_encodes_sentinel() {
        let header = FrameHeader {
            global_seq: 1,
            schema_version: 1,
            event_type_len: EventTypeLen(0),
            metadata_len: None,
        };
        let mut buf = fresh_buf();
        header.write_into(&mut buf);
        let mut ml = [0u8; 4];
        ml.copy_from_slice(&buf[META_LEN_OFFSET..META_LEN_OFFSET + 4]);
        // Invariant: None metadata serializes to the absent sentinel,
        // distinguishing it from Some(empty).
        assert_eq!(u32::from_le_bytes(ml), META_LEN_ABSENT);
        let read = FrameHeader::read_from(&buf).expect("read back");
        assert!(read.metadata_len.is_none());
    }

    #[test]
    fn frame_header_some_zero_metadata_distinct_from_none() {
        // Some(0) — empty metadata field — must NOT encode as the
        // absent sentinel.
        let with_empty = FrameHeader {
            global_seq: 1,
            schema_version: 1,
            event_type_len: EventTypeLen(0),
            metadata_len: Some(MetadataLen(0)),
        };
        let mut buf = fresh_buf();
        with_empty.write_into(&mut buf);
        let mut ml = [0u8; 4];
        ml.copy_from_slice(&buf[META_LEN_OFFSET..META_LEN_OFFSET + 4]);
        assert_eq!(u32::from_le_bytes(ml), 0);
        assert_ne!(u32::from_le_bytes(ml), META_LEN_ABSENT);

        let read = FrameHeader::read_from(&buf).expect("read back");
        assert_eq!(read.metadata_len.map(MetadataLen::as_u32), Some(0));
    }

    #[test]
    fn frame_header_read_from_rejects_buffer_below_size() {
        // Every length in [0, SIZE) must be rejected with ValueTooShort.
        for too_short_len in 0..FrameHeader::SIZE {
            let buf = vec![0u8; too_short_len];
            match FrameHeader::read_from(&buf) {
                Err(DecodeError::ValueTooShort { min, actual }) => {
                    assert_eq!(min, FrameHeader::SIZE);
                    assert_eq!(actual, too_short_len);
                }
                other => panic!("expected ValueTooShort for len={too_short_len}, got {other:?}"),
            }
        }
    }

    #[test]
    fn frame_header_read_from_accepts_exactly_size() {
        // SIZE-byte buffer is the minimum that succeeds.
        let buf = vec![0u8; FrameHeader::SIZE];
        // Default zeros: meta_len bytes are 0 (Some(0), not None).
        let header = FrameHeader::read_from(&buf).expect("accepts at SIZE");
        assert_eq!(header.global_seq, 0);
        assert_eq!(header.schema_version, 0);
        assert_eq!(header.event_type_len.as_u16(), 0);
        assert_eq!(header.metadata_len.map(MetadataLen::as_u32), Some(0));
    }

    proptest! {
        #[test]
        fn frame_header_round_trip(
            global_seq in u64_strategy(),
            schema_version in u32_strategy(),
            et_raw in u16_strategy(),
            meta_choice in 0u32..4,
        ) {
            // meta_choice selects: None, Some(0), Some(MAX-1=u32::MAX-2), Some(arbitrary <= u32::MAX-1).
            let metadata_len = match meta_choice {
                0 => None,
                1 => Some(MetadataLen(0)),
                2 => Some(MetadataLen(u32::MAX - 2)),
                _ => Some(MetadataLen((u32::MAX - 1) / 2)),
            };
            let original = FrameHeader {
                global_seq,
                schema_version,
                event_type_len: EventTypeLen(et_raw),
                metadata_len,
            };
            let mut buf = fresh_buf();
            original.write_into(&mut buf);
            prop_assert_eq!(buf.len(), FrameHeader::SIZE);

            let read = FrameHeader::read_from(&buf).expect("round-trip read");
            prop_assert_eq!(read.global_seq, original.global_seq);
            prop_assert_eq!(read.schema_version, original.schema_version);
            prop_assert_eq!(read.event_type_len.as_u16(), original.event_type_len.as_u16());
            prop_assert_eq!(
                read.metadata_len.map(MetadataLen::as_u32),
                original.metadata_len.map(MetadataLen::as_u32),
            );
        }
    }

    // -----------------------------------------------------------------
    // FrameLayout — structural invariants
    // -----------------------------------------------------------------

    #[test]
    fn layout_concrete_no_metadata_example() {
        // Anchored example to nail down the exact arithmetic the
        // proptest checks structurally: pre_payload = 18 + 2 = 20;
        // padding = 12; payload starts at 32.
        let layout = FrameLayout::compute(
            EventTypeLen::try_from(2usize).unwrap(),
            None,
            PayloadLen::try_from(1usize).unwrap(),
        )
        .expect("ok");
        assert_eq!(layout.padding, 12);
        assert_eq!(layout.event_type, 18..20);
        assert_eq!(layout.metadata, None);
        assert_eq!(layout.payload, 32..33);
        assert_eq!(layout.total, 33);
    }

    #[test]
    fn layout_concrete_with_metadata_example() {
        // pre_payload = 18 + 2 + 3 = 23; padding = 9; payload starts at 32.
        let layout = FrameLayout::compute(
            EventTypeLen::try_from(2usize).unwrap(),
            Some(MetadataLen::try_from(3usize).unwrap()),
            PayloadLen::try_from(4usize).unwrap(),
        )
        .expect("ok");
        assert_eq!(layout.event_type, 18..20);
        assert_eq!(layout.metadata, Some(20..23));
        assert_eq!(layout.padding, 9);
        assert_eq!(layout.payload, 32..36);
        assert_eq!(layout.total, 36);
    }

    proptest! {
        #[test]
        fn layout_structural_invariants(
            et_len_raw in frame_body_length(),
            meta in prop::option::of(frame_body_length()),
            payload_len in frame_body_length(),
        ) {
            // Cap event_type at its actual ceiling.
            let et_len = et_len_raw.min(MAX_EVENT_TYPE_LEN);
            let layout = FrameLayout::compute(
                EventTypeLen::try_from(et_len).unwrap(),
                meta.map(|n| MetadataLen::try_from(n).unwrap()),
                PayloadLen::try_from(payload_len).unwrap(),
            ).expect("bounded inputs compute");

            // I1: event_type starts immediately after the fixed header.
            prop_assert_eq!(
                usize::try_from(layout.event_type.start).unwrap(),
                HEADER_FIXED_SIZE,
            );

            // I2: each variable-width range has length equal to its input.
            prop_assert_eq!(
                (layout.event_type.end - layout.event_type.start) as usize,
                et_len,
            );
            match (meta, layout.metadata.clone()) {
                (None, None) => {},
                (Some(meta_len), Some(range)) => {
                    prop_assert_eq!((range.end - range.start) as usize, meta_len);
                }
                _ => prop_assert!(false, "metadata Option mismatch between input and layout"),
            }
            prop_assert_eq!(
                (layout.payload.end - layout.payload.start) as usize,
                payload_len,
            );

            // I3: ranges are non-overlapping and properly ordered.
            if let Some(m) = layout.metadata.clone() {
                prop_assert!(layout.event_type.end <= m.start);
                prop_assert!(m.end <= layout.payload.start);
            } else {
                prop_assert!(layout.event_type.end <= layout.payload.start);
            }

            // I4: payload starts on a PAYLOAD_ALIGN boundary
            //     (the wire-format invariant zero-copy decoders rely on).
            let payload_start = usize::try_from(layout.payload.start).unwrap();
            prop_assert!(payload_start.is_multiple_of(PAYLOAD_ALIGN));

            // I5: padding < align — the alignment math produces the
            //     minimum padding, never more than align - 1.
            prop_assert!(layout.padding < PAYLOAD_ALIGN);

            // I6: total == payload.end as usize.
            prop_assert_eq!(layout.total, usize::try_from(layout.payload.end).unwrap());

            // I7: total accounts exactly for header + bodies + padding.
            let body_total = et_len
                + meta.unwrap_or(0)
                + layout.padding
                + payload_len;
            prop_assert_eq!(layout.total, HEADER_FIXED_SIZE + body_total);
        }
    }

    // -----------------------------------------------------------------
    // plan / execute
    // -----------------------------------------------------------------

    #[test]
    fn plan_then_execute_matches_encode_frame_concrete() {
        // Anchored equivalence; the proptest below generalizes.
        let one_shot = encode_frame(7, 2, "Evt", Some(b"meta"), b"payload").expect("ok");
        let staged = execute(plan(7, 2, "Evt", Some(b"meta"), b"payload").expect("plan ok"));
        assert_eq!(one_shot.value.as_ref(), staged.value.as_ref());
        assert_eq!(one_shot.offsets.event_type, staged.offsets.event_type);
        assert_eq!(one_shot.offsets.metadata, staged.offsets.metadata);
        assert_eq!(one_shot.offsets.payload, staged.offsets.payload);
    }

    // plan() must surface each WireError variant from the right input.

    #[test]
    fn plan_surfaces_event_type_too_long() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN + 1);
        match plan(1, 1, &huge, None, b"") {
            Err(WireError::EventTypeTooLong { actual, max }) => {
                assert_eq!(actual, MAX_EVENT_TYPE_LEN + 1);
                assert_eq!(max, MAX_EVENT_TYPE_LEN);
            }
            other => panic!("expected EventTypeTooLong, got {other:?}"),
        }
    }

    #[test]
    fn plan_surfaces_metadata_too_long_at_sentinel() {
        // Building a real u32::MAX-byte slice is infeasible — but we can
        // verify the error path via the newtype which is what plan() uses.
        // The "encode_frame at the sentinel" route is checked indirectly.
        match MetadataLen::try_from(MAX_METADATA_LEN + 1) {
            Err(WireError::MetadataTooLong { actual, max }) => {
                assert_eq!(actual, MAX_METADATA_LEN + 1);
                assert_eq!(max, MAX_METADATA_LEN);
            }
            other => panic!("expected MetadataTooLong from newtype, got {other:?}"),
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn plan_surfaces_payload_too_long_via_newtype() {
        // The newtype is the single point of payload-length validation
        // plan() relies on. Building a real u32::MAX+1-byte slice is
        // infeasible; we exercise the rejection path through the same
        // type plan() uses.
        //
        // On 32-bit platforms there is no usize > MAX_PAYLOAD_LEN, so
        // the rejection path is structurally unreachable there and the
        // test is gated to 64-bit pointer widths.
        let over_max: usize = usize::MAX;
        match PayloadLen::try_from(over_max) {
            Err(WireError::PayloadTooLong { actual, max }) => {
                assert!(actual > MAX_PAYLOAD_LEN);
                assert_eq!(max, MAX_PAYLOAD_LEN);
            }
            other => panic!("expected PayloadTooLong, got {other:?}"),
        }
    }

    // execute() invariants.

    #[test]
    fn execute_buffer_length_equals_layout_total() {
        for (et, meta, payload) in [
            ("", None::<&[u8]>, b"" as &[u8]),
            ("X", None, b""),
            ("Evt", Some(b"meta".as_slice()), b"payload"),
            ("LongerType", Some(b"".as_slice()), b"x"),
        ] {
            let p = plan(1, 1, et, meta, payload).expect("plan ok");
            let total = p.layout.total;
            let frame = execute(p);
            assert_eq!(frame.value.len(), total);
        }
    }

    #[test]
    fn execute_padding_bytes_are_zero() {
        // Choose inputs where padding > 0: 18 + 1 (et) = 19, padding = 13.
        let frame = encode_frame(1, 1, "x", None, b"payload").expect("ok");
        let pad_start = usize::try_from(frame.offsets.event_type.end).unwrap();
        let pad_end = usize::try_from(frame.offsets.payload.start).unwrap();
        assert!(pad_end > pad_start, "expected at least one padding byte");
        for (i, byte) in frame.value[pad_start..pad_end].iter().enumerate() {
            assert_eq!(
                *byte,
                0,
                "padding byte at offset {} is {:#x}",
                pad_start + i,
                byte
            );
        }
    }

    proptest! {
        #[test]
        fn plan_execute_equals_encode_frame(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let one_shot = encode_frame(
                global_seq, schema_version, &event_type, meta_ref, &payload,
            ).expect("valid inputs encode");
            let staged = execute(
                plan(global_seq, schema_version, &event_type, meta_ref, &payload)
                    .expect("valid inputs plan"),
            );
            // Whole-buffer equality is the strongest equivalence.
            prop_assert_eq!(one_shot.value.as_ref(), staged.value.as_ref());
            prop_assert_eq!(one_shot.offsets.event_type, staged.offsets.event_type);
            prop_assert_eq!(one_shot.offsets.metadata, staged.offsets.metadata);
            prop_assert_eq!(one_shot.offsets.payload, staged.offsets.payload);
        }

        #[test]
        fn execute_invariants(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let meta_ref = metadata.as_deref();
            let p = plan(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("valid inputs plan");
            let layout_total = p.layout.total;
            let event_type_range = p.layout.event_type.clone();
            let metadata_range = p.layout.metadata.clone();
            let payload_range = p.layout.payload.clone();
            let frame = execute(p);

            // I1: buffer length equals layout.total.
            prop_assert_eq!(frame.value.len(), layout_total);

            // I2: payload pointer is 16-byte aligned.
            let payload_slice_start = usize::try_from(payload_range.start).unwrap();
            let ptr = frame.value[payload_slice_start..].as_ptr().addr();
            prop_assert!(ptr.is_multiple_of(PAYLOAD_ALIGN));

            // I3: each body byte lands at its layout offset.
            let et_start = usize::try_from(event_type_range.start).unwrap();
            let et_end = usize::try_from(event_type_range.end).unwrap();
            prop_assert_eq!(&frame.value[et_start..et_end], event_type.as_bytes());
            if let (Some(range), Some(meta)) = (metadata_range.clone(), meta_ref) {
                let s = usize::try_from(range.start).unwrap();
                let e = usize::try_from(range.end).unwrap();
                prop_assert_eq!(&frame.value[s..e], meta);
            }
            let p_start = usize::try_from(payload_range.start).unwrap();
            let p_end = usize::try_from(payload_range.end).unwrap();
            prop_assert_eq!(&frame.value[p_start..p_end], payload.as_slice());

            // I4: padding bytes (between event_type/metadata end and payload start) are zero.
            let pad_start = metadata_range
                .as_ref()
                .map_or(et_end, |r| usize::try_from(r.end).unwrap());
            for byte in &frame.value[pad_start..p_start] {
                prop_assert_eq!(*byte, 0u8);
            }
        }
    }
}
