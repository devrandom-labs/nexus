//! Wire-format row builder shared by `nexus-fjall` and `nexus-store::testing`.
//!
//! One canonical implementation of the on-disk row format: a fixed
//! header, then event-type bytes, optional metadata bytes, alignment
//! padding, and finally payload. The padding makes the payload pointer
//! 16-byte aligned in the resulting [`Bytes`] buffer, which is a
//! wire-format invariant — every adapter must use this function to
//! encode rows, every decoder may rely on the alignment.
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

use aligned_vec::{AVec, ConstAlign};
use bytes::Bytes;
use core::ops::Range;
use thiserror::Error;

/// Payload alignment in bytes. Wire-format invariant.
pub const PAYLOAD_ALIGN: usize = 16;

/// Fixed header size: `global_seq (8) + schema_version (4) + et_len (2) + meta_len (4)`.
pub const HEADER_FIXED_SIZE: usize = 18;

/// Offset of the `global_seq` field in the row's header.
pub const GLOBAL_SEQ_OFFSET: usize = 0;

/// Offset of the `schema_version` field in the row's header.
pub const SCHEMA_VERSION_OFFSET: usize = 8;

/// Offset of the `event_type_len` field in the row's header.
pub const EVENT_TYPE_LEN_OFFSET: usize = 12;

/// Offset of the `meta_len` field in the row's header.
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

/// Output of [`build_row`]: the assembled buffer plus byte ranges into it.
#[derive(Debug)]
pub struct RowBuilt {
    pub value: Bytes,
    pub offsets: RowOffsets,
}

/// Byte ranges for each variable-width field within a [`RowBuilt::value`] buffer.
///
/// Fixed-position header fields (`global_seq`, `schema_version`,
/// `event_type_len`, `meta_len`) are read from constant offsets and
/// have no ranges here.
#[derive(Debug, Clone)]
pub struct RowOffsets {
    pub event_type: Range<u32>,
    pub metadata: Option<Range<u32>>,
    pub payload: Range<u32>,
}

/// Errors from [`build_row`].
#[derive(Debug, Error)]
pub enum WireError {
    #[error("event type length {actual} exceeds maximum {max}")]
    EventTypeTooLong { actual: usize, max: usize },
    #[error("metadata length {actual} exceeds maximum {max}")]
    MetadataTooLong { actual: usize, max: usize },
    #[error("payload length {actual} exceeds maximum {max}")]
    PayloadTooLong { actual: usize, max: usize },
    #[error("row length overflow combining header={header}, padding={padding}, payload={payload}")]
    RowLengthOverflow {
        header: usize,
        padding: usize,
        payload: usize,
    },
}

/// Output of [`decode_row`]: header fields plus byte ranges into the input value.
#[derive(Debug)]
pub struct DecodedRow {
    pub global_seq: u64,
    pub schema_version: u32,
    pub offsets: RowOffsets,
}

/// Errors from [`decode_row`].
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

/// Build one row buffer with payload aligned to [`PAYLOAD_ALIGN`].
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
/// the assembled row would overflow `usize` on the target platform.
#[allow(
    clippy::too_many_lines,
    reason = "single-pass row construction is intrinsically linear; splitting helpers would obscure the layout invariant"
)]
pub fn build_row(
    global_seq: u64,
    schema_version: u32,
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<RowBuilt, WireError> {
    let event_type_bytes = event_type.as_bytes();
    let event_type_len = event_type_bytes.len();
    if event_type_len > MAX_EVENT_TYPE_LEN {
        return Err(WireError::EventTypeTooLong {
            actual: event_type_len,
            max: MAX_EVENT_TYPE_LEN,
        });
    }

    let meta_bytes_len = metadata.map_or(0, <[u8]>::len);
    if meta_bytes_len > MAX_METADATA_LEN {
        return Err(WireError::MetadataTooLong {
            actual: meta_bytes_len,
            max: MAX_METADATA_LEN,
        });
    }

    let payload_len = payload.len();
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLong {
            actual: payload_len,
            max: MAX_PAYLOAD_LEN,
        });
    }

    // Pre-padding layout: [fixed header (18)][event_type][metadata?].
    let pre_payload_len = HEADER_FIXED_SIZE
        .checked_add(event_type_len)
        .and_then(|n| n.checked_add(meta_bytes_len))
        .ok_or_else(|| WireError::RowLengthOverflow {
            header: HEADER_FIXED_SIZE.saturating_add(event_type_len),
            padding: 0,
            payload: payload_len,
        })?;

    let padding = (PAYLOAD_ALIGN - (pre_payload_len % PAYLOAD_ALIGN)) % PAYLOAD_ALIGN;

    let total = pre_payload_len
        .checked_add(padding)
        .and_then(|n| n.checked_add(payload_len))
        .ok_or(WireError::RowLengthOverflow {
            header: pre_payload_len,
            padding,
            payload: payload_len,
        })?;

    let mut buf: AVec<u8, ConstAlign<PAYLOAD_ALIGN>> = AVec::with_capacity(PAYLOAD_ALIGN, total);

    let event_type_len_u16 =
        u16::try_from(event_type_len).map_err(|_| WireError::EventTypeTooLong {
            actual: event_type_len,
            max: MAX_EVENT_TYPE_LEN,
        })?;
    let meta_len_field = match metadata {
        Some(_) => u32::try_from(meta_bytes_len).map_err(|_| WireError::MetadataTooLong {
            actual: meta_bytes_len,
            max: MAX_METADATA_LEN,
        })?,
        None => META_LEN_ABSENT,
    };

    buf.extend_from_slice(&global_seq.to_le_bytes());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&event_type_len_u16.to_le_bytes());
    buf.extend_from_slice(&meta_len_field.to_le_bytes());
    buf.extend_from_slice(event_type_bytes);
    if let Some(m) = metadata {
        buf.extend_from_slice(m);
    }
    buf.resize(buf.len() + padding, 0u8);
    buf.extend_from_slice(payload);

    let event_type_start =
        u32::try_from(HEADER_FIXED_SIZE).map_err(|_| WireError::RowLengthOverflow {
            header: HEADER_FIXED_SIZE,
            padding,
            payload: payload_len,
        })?;
    let event_type_end = event_type_start
        + u32::try_from(event_type_len).map_err(|_| WireError::EventTypeTooLong {
            actual: event_type_len,
            max: MAX_EVENT_TYPE_LEN,
        })?;
    let metadata_range = metadata
        .map(|_| -> Result<Range<u32>, WireError> {
            let start = event_type_end;
            let end = start
                + u32::try_from(meta_bytes_len).map_err(|_| WireError::MetadataTooLong {
                    actual: meta_bytes_len,
                    max: MAX_METADATA_LEN,
                })?;
            Ok(start..end)
        })
        .transpose()?;
    let payload_start =
        u32::try_from(pre_payload_len + padding).map_err(|_| WireError::RowLengthOverflow {
            header: pre_payload_len,
            padding,
            payload: payload_len,
        })?;
    let payload_end = payload_start
        + u32::try_from(payload_len).map_err(|_| WireError::PayloadTooLong {
            actual: payload_len,
            max: MAX_PAYLOAD_LEN,
        })?;

    Ok(RowBuilt {
        value: Bytes::from_owner(buf),
        offsets: RowOffsets {
            event_type: event_type_start..event_type_end,
            metadata: metadata_range,
            payload: payload_start..payload_end,
        },
    })
}

/// Decode a row value built by [`build_row`].
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
#[allow(
    clippy::too_many_lines,
    reason = "single-pass header + variable-field decode is intrinsically linear"
)]
pub fn decode_row(value: &[u8]) -> Result<DecodedRow, DecodeError> {
    if value.len() < HEADER_FIXED_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: HEADER_FIXED_SIZE,
            actual: value.len(),
        });
    }

    // The slice indexing below is bounds-checked by the explicit length
    // guard above (`value.len() >= HEADER_FIXED_SIZE`); fixed-size array
    // construction reads each byte individually so no fallible try_into
    // sits in the hot path.
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
    let et_len = usize::from(u16::from_le_bytes([
        value[EVENT_TYPE_LEN_OFFSET],
        value[EVENT_TYPE_LEN_OFFSET + 1],
    ]));
    let meta_len = u32::from_le_bytes([
        value[META_LEN_OFFSET],
        value[META_LEN_OFFSET + 1],
        value[META_LEN_OFFSET + 2],
        value[META_LEN_OFFSET + 3],
    ]);

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

    let (metadata_range, post_meta) = if meta_len == META_LEN_ABSENT {
        (None, et_end)
    } else {
        let meta_len_usize =
            usize::try_from(meta_len).map_err(|_| DecodeError::OffsetOverflow {
                value_len: value.len(),
            })?;
        let meta_end = et_end
            .checked_add(meta_len_usize)
            .ok_or(DecodeError::OffsetOverflow {
                value_len: value.len(),
            })?;
        if value.len() < meta_end {
            return Err(DecodeError::MetadataTruncated {
                meta_len,
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
    };

    let padding = (PAYLOAD_ALIGN - (post_meta % PAYLOAD_ALIGN)) % PAYLOAD_ALIGN;
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

    Ok(DecodedRow {
        global_seq,
        schema_version,
        offsets: RowOffsets {
            event_type: et_start_u32..et_end_u32,
            metadata: metadata_range,
            payload: payload_start_u32..payload_end_u32,
        },
    })
}

#[cfg(test)]
#[allow(
    clippy::as_conversions,
    clippy::redundant_clone,
    clippy::single_match_else,
    reason = "test code: index arithmetic and prop_assert_eq macro expansions"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn payload_ptr_aligned(row: &RowBuilt) -> bool {
        let start = usize::try_from(row.offsets.payload.start).expect("u32 fits usize");
        let end = usize::try_from(row.offsets.payload.end).expect("u32 fits usize");
        let payload_slice = &row.value[start..end];
        payload_slice.as_ptr().addr().is_multiple_of(PAYLOAD_ALIGN)
    }

    proptest! {
        #[test]
        fn payload_pointer_is_16_aligned(
            global_seq in any::<u64>(),
            schema_version in any::<u32>(),
            event_type in "[a-zA-Z]{0,256}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..1024)),
            payload in prop::collection::vec(any::<u8>(), 0..4096),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("build_row succeeds on bounded inputs");
            prop_assert!(payload_ptr_aligned(&row));
        }

        #[test]
        fn ranges_recover_each_field(
            global_seq in any::<u64>(),
            schema_version in any::<u32>(),
            event_type in "[a-zA-Z]{1,128}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..256)),
            payload in prop::collection::vec(any::<u8>(), 1..1024),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("build_row succeeds on bounded inputs");
            let v = &row.value;
            prop_assert_eq!(
                &v[row.offsets.event_type.start as usize..row.offsets.event_type.end as usize],
                event_type.as_bytes()
            );
            prop_assert_eq!(
                &v[row.offsets.payload.start as usize..row.offsets.payload.end as usize],
                payload.as_slice()
            );
            if let (Some(meta), Some(range)) = (meta_ref, row.offsets.metadata) {
                prop_assert_eq!(
                    &v[range.start as usize..range.end as usize],
                    meta
                );
            }
        }

        #[test]
        fn header_fields_are_recoverable(
            global_seq in any::<u64>(),
            schema_version in any::<u32>(),
            event_type in "[a-zA-Z]{0,64}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..64)),
            payload in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("build_row succeeds on bounded inputs");
            let v = &row.value;

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
        let row = build_row(1, 1, "X", None, &[]).expect("trivial row builds");
        assert!(payload_ptr_aligned(&row));
        assert_eq!(row.offsets.payload.start, row.offsets.payload.end);
    }

    #[test]
    fn empty_event_type_permitted() {
        let row =
            build_row(1, 1, "", None, b"data").expect("empty event_type accepted at wire layer");
        assert!(payload_ptr_aligned(&row));
    }

    #[test]
    fn max_event_type_accepted() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN);
        build_row(1, 1, &huge, None, b"d").expect("max-length event_type accepted");
    }

    #[test]
    fn over_max_event_type_rejected() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN + 1);
        assert!(matches!(
            build_row(1, 1, &huge, None, b"d"),
            Err(WireError::EventTypeTooLong { .. })
        ));
    }

    #[test]
    fn meta_len_u32_max_is_absent_sentinel() {
        let row = build_row(1, 1, "X", None, b"d").expect("none-metadata row builds");
        let mut ml_buf = [0u8; 4];
        ml_buf.copy_from_slice(&row.value[META_LEN_OFFSET..META_LEN_OFFSET + 4]);
        assert_eq!(u32::from_le_bytes(ml_buf), META_LEN_ABSENT);
        assert!(row.offsets.metadata.is_none());
    }

    proptest! {
        #[test]
        fn build_then_decode_round_trips(
            global_seq in any::<u64>(),
            schema_version in any::<u32>(),
            event_type in "[a-zA-Z]{0,128}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..256)),
            payload in prop::collection::vec(any::<u8>(), 0..1024),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(global_seq, schema_version, &event_type, meta_ref, &payload)
                .expect("build_row succeeds on bounded inputs");
            let decoded = decode_row(&row.value).expect("decode_row succeeds on a built row");
            prop_assert_eq!(decoded.global_seq, global_seq);
            prop_assert_eq!(decoded.schema_version, schema_version);
            prop_assert_eq!(decoded.offsets.event_type.clone(), row.offsets.event_type.clone());
            prop_assert_eq!(decoded.offsets.metadata.clone(), row.offsets.metadata.clone());
            prop_assert_eq!(decoded.offsets.payload.clone(), row.offsets.payload.clone());
        }
    }

    #[test]
    fn decode_rejects_truncated_value() {
        let too_short = vec![0u8; HEADER_FIXED_SIZE - 1];
        assert!(matches!(
            decode_row(&too_short),
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
            decode_row(&buf),
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
            decode_row(&buf),
            Err(DecodeError::MetadataTruncated { .. })
        ));
    }
}
