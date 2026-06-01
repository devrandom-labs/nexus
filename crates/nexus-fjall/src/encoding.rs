use thiserror::Error;

/// Errors from decoding stored byte layouts.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },

    #[error("invalid UTF-8 in event type")]
    InvalidUtf8(#[source] std::str::Utf8Error),

    #[error("metadata range exceeds buffer (meta_len={meta_len}, total={total})")]
    MetadataOutOfBounds { meta_len: u32, total: usize },
}

/// Errors from encoding values into byte layouts.
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("event type too long: {len} bytes (max {})", u16::MAX)]
    EventTypeTooLong { len: usize },

    #[error("stream ID too long: {len} bytes (max {})", u16::MAX)]
    IdTooLong { len: usize },

    #[error("metadata too long: {len} bytes (max {})", u32::MAX - 1)]
    MetadataTooLong { len: usize },
}

/// Size of the event key header: `[u16 BE id_len]`.
const EVENT_KEY_HEADER_SIZE: usize = 2;

/// Size of the version suffix in an event key: `[u64 BE version]`.
const EVENT_KEY_VERSION_SIZE: usize = 8;

/// Size of an encoded stream version value in bytes: `[u64 LE version]`.
pub const STREAM_VERSION_SIZE: usize = 8;

/// Size of the fixed header in an encoded event value:
/// `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len]`.
pub const EVENT_VALUE_HEADER_SIZE: usize = 18;

/// Sentinel value for `meta_len` indicating no metadata.
/// Distinguishes `None` from `Some(empty)`; the encoder rejects
/// metadata exactly `u32::MAX` bytes long to avoid collision.
pub const META_LEN_ABSENT: u32 = u32::MAX;

/// Compute the total size of an event key for a given ID length.
///
/// Format: `[u16 BE id_len][id_bytes][u64 BE version]`.
#[must_use]
pub const fn event_key_size(id_len: usize) -> usize {
    EVENT_KEY_HEADER_SIZE + id_len + EVENT_KEY_VERSION_SIZE
}

/// Encode an event key as `[u16 BE id_len][id_bytes][u64 BE version]`.
///
/// The length prefix prevents prefix collisions in range scans between
/// IDs where one is a byte-prefix of another (e.g. `"foo"` vs `"foobar"`).
/// Big-endian encoding ensures natural byte ordering in LSM trees.
///
/// # Errors
///
/// Returns [`EncodeError::IdTooLong`] if `id_bytes` exceeds `u16::MAX` bytes.
pub fn encode_event_key(id_bytes: &[u8], version: u64) -> Result<Vec<u8>, EncodeError> {
    let id_len = id_bytes.len();
    let id_len_u16 = u16::try_from(id_len).map_err(|_| EncodeError::IdTooLong { len: id_len })?;

    let total = event_key_size(id_len);
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(&id_len_u16.to_be_bytes());
    buf.extend_from_slice(id_bytes);
    buf.extend_from_slice(&version.to_be_bytes());
    Ok(buf)
}

/// Decode an event key from `[u16 BE id_len][id_bytes][u64 BE version]`.
///
/// Returns the ID bytes slice and the version.
///
/// # Errors
///
/// Returns [`DecodeError::ValueTooShort`] if `key` is shorter than the header,
/// or if it doesn't contain enough bytes for the claimed ID length + version.
pub fn decode_event_key(key: &[u8]) -> Result<(&[u8], u64), DecodeError> {
    let min_header = EVENT_KEY_HEADER_SIZE;
    if key.len() < min_header {
        return Err(DecodeError::ValueTooShort {
            min: min_header,
            actual: key.len(),
        });
    }

    let id_len = usize::from(u16::from_be_bytes([key[0], key[1]]));
    let expected_total = event_key_size(id_len);
    if key.len() != expected_total {
        return Err(DecodeError::InvalidSize {
            expected: expected_total,
            actual: key.len(),
        });
    }

    let id_bytes = &key[EVENT_KEY_HEADER_SIZE..EVENT_KEY_HEADER_SIZE + id_len];
    let version_start = EVENT_KEY_HEADER_SIZE + id_len;
    let version = u64::from_be_bytes([
        key[version_start],
        key[version_start + 1],
        key[version_start + 2],
        key[version_start + 3],
        key[version_start + 4],
        key[version_start + 5],
        key[version_start + 6],
        key[version_start + 7],
    ]);

    Ok((id_bytes, version))
}

/// Encode a stream version as `[u64 LE version]`.
///
/// Little-endian encoding is used since stream metadata has no ordering requirement.
#[must_use]
pub const fn encode_stream_version(version: u64) -> [u8; STREAM_VERSION_SIZE] {
    version.to_le_bytes()
}

/// Decode a stream version from `[u64 LE version]`.
///
/// # Errors
///
/// Returns [`DecodeError::InvalidSize`] if `value` is not exactly [`STREAM_VERSION_SIZE`] bytes.
pub fn decode_stream_version(value: &[u8]) -> Result<u64, DecodeError> {
    if value.len() != STREAM_VERSION_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: STREAM_VERSION_SIZE,
            actual: value.len(),
        });
    }

    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

/// Decoded event value with `Range<u32>` offsets into the original buffer.
///
/// All three ranges are valid offsets into the `&Bytes` that was passed
/// to `decode_event_value`. `metadata_range` is `None` when the encoded
/// `meta_len` is the `META_LEN_ABSENT` sentinel.
#[derive(Debug)]
pub struct DecodedEvent {
    pub global_seq: u64,
    pub schema_version: u32,
    pub event_type_range: std::ops::Range<u32>,
    pub payload_range: std::ops::Range<u32>,
    pub metadata_range: Option<std::ops::Range<u32>>,
}

/// Encode an event value as
/// `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len][event_type UTF-8][meta][payload]`.
///
/// `meta_len == META_LEN_ABSENT` (`u32::MAX`) when `metadata.is_none()`.
/// The encoder rejects metadata of exactly `u32::MAX` bytes to avoid
/// collision with the absent sentinel.
///
/// The buffer is cleared and reused (no reallocation if capacity suffices).
///
/// # Errors
///
/// - [`EncodeError::EventTypeTooLong`] if `event_type` exceeds `u16::MAX` bytes.
/// - [`EncodeError::MetadataTooLong`] if `metadata` is `Some(_)` with length >= `u32::MAX`.
#[allow(
    clippy::too_many_arguments,
    reason = "encoding all event fields requires all 6 parameters; \
              a struct would add allocation/overhead for a low-level hot path"
)]
pub fn encode_event_value(
    buf: &mut Vec<u8>,
    global_seq: u64,
    schema_version: u32,
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<(), EncodeError> {
    let event_type_len = event_type.len();
    if event_type_len > usize::from(u16::MAX) {
        return Err(EncodeError::EventTypeTooLong {
            len: event_type_len,
        });
    }
    let type_len_u16 =
        u16::try_from(event_type_len).map_err(|_| EncodeError::EventTypeTooLong {
            len: event_type_len,
        })?;

    let (meta_len_field, meta_bytes_len): (u32, usize) = match metadata {
        None => (META_LEN_ABSENT, 0),
        Some(m) => {
            let len = m.len();
            let len_u32 = u32::try_from(len).map_err(|_| EncodeError::MetadataTooLong { len })?;
            if len_u32 == META_LEN_ABSENT {
                return Err(EncodeError::MetadataTooLong { len });
            }
            (len_u32, len)
        }
    };

    buf.clear();
    let total = EVENT_VALUE_HEADER_SIZE + event_type_len + meta_bytes_len + payload.len();
    buf.reserve(total);

    buf.extend_from_slice(&global_seq.to_le_bytes());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&type_len_u16.to_le_bytes());
    buf.extend_from_slice(&meta_len_field.to_le_bytes());
    buf.extend_from_slice(event_type.as_bytes());
    if let Some(m) = metadata {
        buf.extend_from_slice(m);
    }
    buf.extend_from_slice(payload);
    Ok(())
}

/// Decode an event value into ranges over the supplied `Bytes` buffer.
///
/// The returned `DecodedEvent` carries `Range<u32>` offsets, not slices —
/// callers construct `PersistedEnvelope` from the same `Bytes` + the ranges,
/// preserving the Arc-shared zero-copy invariant.
///
/// # Errors
///
/// - [`DecodeError::ValueTooShort`] if the header or any length-prefixed region is truncated.
/// - [`DecodeError::InvalidUtf8`] if `event_type` bytes are not valid UTF-8.
/// - [`DecodeError::MetadataOutOfBounds`] if `meta_len` claims bytes past the buffer end.
/// - [`DecodeError::InvalidSize`] if any computed offset would exceed `u32::MAX` (cannot fit in `Range<u32>`).
pub fn decode_event_value(value: &bytes::Bytes) -> Result<DecodedEvent, DecodeError> {
    if value.len() < EVENT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: EVENT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }

    let global_seq = u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]);
    let schema_version = u32::from_le_bytes([value[8], value[9], value[10], value[11]]);
    let event_type_len = u16::from_le_bytes([value[12], value[13]]);
    let meta_len = u32::from_le_bytes([value[14], value[15], value[16], value[17]]);

    let event_type_start = EVENT_VALUE_HEADER_SIZE;
    let event_type_end = event_type_start + usize::from(event_type_len);
    if value.len() < event_type_end {
        return Err(DecodeError::ValueTooShort {
            min: event_type_end,
            actual: value.len(),
        });
    }

    // UTF-8 validation of event_type at decode (PersistedEnvelope::try_new
    // also validates; this gives early error before envelope construction).
    std::str::from_utf8(&value[event_type_start..event_type_end])
        .map_err(DecodeError::InvalidUtf8)?;

    let (metadata_range, payload_start) = if meta_len == META_LEN_ABSENT {
        (None, event_type_end)
    } else {
        let meta_start = event_type_end;
        let meta_len_usize =
            usize::try_from(meta_len).map_err(|_| DecodeError::MetadataOutOfBounds {
                meta_len,
                total: value.len(),
            })?;
        let meta_end =
            meta_start
                .checked_add(meta_len_usize)
                .ok_or(DecodeError::MetadataOutOfBounds {
                    meta_len,
                    total: value.len(),
                })?;
        if value.len() < meta_end {
            return Err(DecodeError::MetadataOutOfBounds {
                meta_len,
                total: value.len(),
            });
        }
        let start_u32 = u32::try_from(meta_start).map_err(|_| DecodeError::InvalidSize {
            expected: usize::MAX,
            actual: meta_start,
        })?;
        let end_u32 = u32::try_from(meta_end).map_err(|_| DecodeError::InvalidSize {
            expected: usize::MAX,
            actual: meta_end,
        })?;
        (Some(start_u32..end_u32), meta_end)
    };

    let payload_end = value.len();
    let et_start_u32 = u32::try_from(event_type_start).map_err(|_| DecodeError::InvalidSize {
        expected: usize::MAX,
        actual: event_type_start,
    })?;
    let et_end_u32 = u32::try_from(event_type_end).map_err(|_| DecodeError::InvalidSize {
        expected: usize::MAX,
        actual: event_type_end,
    })?;
    let pl_start_u32 = u32::try_from(payload_start).map_err(|_| DecodeError::InvalidSize {
        expected: usize::MAX,
        actual: payload_start,
    })?;
    let pl_end_u32 = u32::try_from(payload_end).map_err(|_| DecodeError::InvalidSize {
        expected: usize::MAX,
        actual: payload_end,
    })?;

    Ok(DecodedEvent {
        global_seq,
        schema_version,
        event_type_range: et_start_u32..et_end_u32,
        payload_range: pl_start_u32..pl_end_u32,
        metadata_range,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Snapshot encoding
// ═══════════════════════════════════════════════════════════════════════════

/// Size of the snapshot value header: `[u32 LE schema_version][u64 BE version]`.
#[cfg(feature = "snapshot")]
pub(crate) const SNAPSHOT_VALUE_HEADER_SIZE: usize = 12;

/// Encode a snapshot value as `[u32 LE schema_version][u64 BE version][payload]`.
#[cfg(feature = "snapshot")]
pub(crate) fn encode_snapshot_value(
    buf: &mut Vec<u8>,
    schema_version: u32,
    version: u64,
    payload: &[u8],
) {
    buf.clear();
    buf.reserve(SNAPSHOT_VALUE_HEADER_SIZE + payload.len());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&version.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// Decode a snapshot value from `[u32 LE schema_version][u64 BE version][payload]`.
///
/// # Errors
///
/// Returns [`DecodeError::ValueTooShort`] if value is shorter than the header.
#[cfg(feature = "snapshot")]
pub(crate) fn decode_snapshot_value(value: &[u8]) -> Result<(u32, u64, &[u8]), DecodeError> {
    if value.len() < SNAPSHOT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: SNAPSHOT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }
    let schema_version = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let version = u64::from_be_bytes([
        value[4], value[5], value[6], value[7], value[8], value[9], value[10], value[11],
    ]);
    let payload = &value[12..];
    Ok((schema_version, version, payload))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(
    clippy::as_conversions,
    reason = "u32→usize casts in tests are safe on 32-bit+ platforms"
)]
mod tests {
    use super::*;

    // --- Event key tests ---

    #[test]
    fn event_key_round_trips() {
        let id = b"stream-42";
        let version: u64 = 7;
        let encoded = encode_event_key(id, version).unwrap();
        let (decoded_id, decoded_version) = decode_event_key(&encoded).unwrap();
        assert_eq!(decoded_id, id);
        assert_eq!(decoded_version, version);
    }

    #[test]
    fn event_key_is_big_endian_for_ordering() {
        let key_v1 = encode_event_key(b"s1", 1).unwrap();
        let key_v2 = encode_event_key(b"s1", 2).unwrap();
        // Byte-level comparison: version 1 < version 2
        assert!(key_v1 < key_v2);
    }

    #[test]
    fn event_key_length_prefix_prevents_collisions() {
        // "foo" and "foobar" must NOT overlap in range scans.
        // With length prefix, "foo" (len=3) sorts differently from "foobar" (len=6).
        let key_foo_v1 = encode_event_key(b"foo", 1).unwrap();
        let key_foobar_v1 = encode_event_key(b"foobar", 1).unwrap();
        // They must not be equal, and the shorter ID sorts first (0x0003 < 0x0006).
        assert!(key_foo_v1 < key_foobar_v1);

        // Range scan for "foo" (from v1 to vMAX) must NOT include "foobar" events.
        let key_foo_vmax = encode_event_key(b"foo", u64::MAX).unwrap();
        assert!(key_foo_vmax < key_foobar_v1);
    }

    #[test]
    fn event_key_empty_id_round_trips() {
        let encoded = encode_event_key(b"", 42).unwrap();
        let (decoded_id, decoded_version) = decode_event_key(&encoded).unwrap();
        assert_eq!(decoded_id, b"");
        assert_eq!(decoded_version, 42);
    }

    #[test]
    fn event_key_decode_rejects_truncated_header() {
        let too_short = [0u8; 1]; // less than 2-byte header
        assert!(decode_event_key(&too_short).is_err());
    }

    #[test]
    fn event_key_decode_rejects_wrong_total_size() {
        // Header says id_len=3, so total should be 2+3+8=13, but we provide 12
        let mut bad = Vec::new();
        bad.extend_from_slice(&3u16.to_be_bytes());
        bad.extend_from_slice(b"foo");
        bad.extend_from_slice(&42u64.to_be_bytes()[..7]); // only 7 version bytes
        assert!(decode_event_key(&bad).is_err());
    }

    // --- Stream version tests ---

    #[test]
    fn stream_version_round_trips() {
        let version: u64 = 999;
        let encoded = encode_stream_version(version);
        let decoded = decode_stream_version(&encoded).unwrap();
        assert_eq!(decoded, version);
    }

    #[test]
    fn stream_version_decode_rejects_wrong_size() {
        let too_short = [0u8; 4];
        assert!(decode_stream_version(&too_short).is_err());

        let too_long = [0u8; 16];
        assert!(decode_stream_version(&too_long).is_err());
    }

    // --- Event value tests ---

    #[test]
    fn event_value_round_trips() {
        let mut buf = Vec::new();
        let global_seq: u64 = 42;
        let schema_version: u32 = 3;
        let event_type = "UserCreated";
        let payload = b"some-json-bytes";

        encode_event_value(
            &mut buf,
            global_seq,
            schema_version,
            event_type,
            None,
            payload,
        )
        .unwrap();
        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

        assert_eq!(decoded.global_seq, global_seq);
        assert_eq!(decoded.schema_version, schema_version);
        assert!(decoded.metadata_range.is_none());
        let et = &bytes_buf
            [decoded.event_type_range.start as usize..decoded.event_type_range.end as usize];
        assert_eq!(et, event_type.as_bytes());
        let pl =
            &bytes_buf[decoded.payload_range.start as usize..decoded.payload_range.end as usize];
        assert_eq!(pl, payload);
    }

    #[test]
    fn event_value_empty_payload() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 7, 1, "Empty", None, b"").unwrap();
        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();
        assert_eq!(decoded.global_seq, 7);
        assert_eq!(decoded.schema_version, 1);
        let et = &bytes_buf
            [decoded.event_type_range.start as usize..decoded.event_type_range.end as usize];
        assert_eq!(et, b"Empty");
        assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
    }

    #[test]
    fn event_value_decode_rejects_truncated_header() {
        let short = bytes::Bytes::from(vec![0u8; 4]); // less than EVENT_VALUE_HEADER_SIZE (18)
        let result = decode_event_value(&short);
        assert!(result.is_err());
    }

    #[test]
    fn event_value_decode_rejects_truncated_event_type() {
        // Build a valid 18-byte header that claims event_type_len = 100 but
        // provide no type bytes.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u64.to_le_bytes()); // global_seq
        buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
        buf.extend_from_slice(&100u16.to_le_bytes()); // event_type_len = 100
        buf.extend_from_slice(&META_LEN_ABSENT.to_le_bytes()); // meta_len
        let bytes_buf = bytes::Bytes::from(buf);
        let result = decode_event_value(&bytes_buf);
        assert!(result.is_err());
    }

    #[test]
    fn event_value_reuses_buffer() {
        let mut buf = Vec::new();

        // First encode
        encode_event_value(&mut buf, 1, 1, "First", None, b"aaa").unwrap();
        let cap_after_first = buf.capacity();

        // Second encode reuses the buffer (clear, no realloc if capacity suffices)
        encode_event_value(&mut buf, 2, 2, "Sec", None, b"bb").unwrap();
        assert_eq!(buf.capacity(), cap_after_first);

        // Verify the second encode is correct (not contaminated by first)
        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();
        assert_eq!(decoded.global_seq, 2);
        assert_eq!(decoded.schema_version, 2);
        let et = &bytes_buf
            [decoded.event_type_range.start as usize..decoded.event_type_range.end as usize];
        assert_eq!(et, b"Sec");
        let pl =
            &bytes_buf[decoded.payload_range.start as usize..decoded.payload_range.end as usize];
        assert_eq!(pl, b"bb");
    }

    // --- New metadata tests ---

    #[test]
    fn event_value_round_trips_with_metadata() {
        let mut buf = Vec::new();
        let global_seq: u64 = 42;
        let schema_version: u32 = 3;
        let event_type = "UserCreated";
        let metadata = b"correlation-abc-123".as_slice();
        let payload = b"some-json-bytes";

        encode_event_value(
            &mut buf,
            global_seq,
            schema_version,
            event_type,
            Some(metadata),
            payload,
        )
        .unwrap();

        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

        assert_eq!(decoded.global_seq, global_seq);
        assert_eq!(decoded.schema_version, schema_version);
        assert_eq!(
            &bytes_buf
                [decoded.event_type_range.start as usize..decoded.event_type_range.end as usize],
            event_type.as_bytes()
        );
        assert_eq!(
            &bytes_buf[decoded.payload_range.start as usize..decoded.payload_range.end as usize],
            payload
        );
        let meta_range = decoded.metadata_range.expect("metadata present");
        assert_eq!(
            &bytes_buf[meta_range.start as usize..meta_range.end as usize],
            metadata
        );
    }

    #[test]
    fn event_value_round_trips_without_metadata() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 7, 1, "Empty", None, b"").unwrap();

        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

        assert_eq!(decoded.global_seq, 7);
        assert_eq!(decoded.schema_version, 1);
        assert!(decoded.metadata_range.is_none());
        assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
    }

    #[test]
    fn meta_len_absent_constant_is_u32_max() {
        // The encoder rejects metadata of length u32::MAX to avoid colliding with
        // the absent sentinel. This test asserts the sentinel value and serves
        // as a regression guard if the constant ever changes.
        assert_eq!(META_LEN_ABSENT, u32::MAX);
    }

    #[test]
    fn event_value_decode_rejects_meta_len_exceeding_buffer() {
        // Manually build a corrupt value: claim meta_len = 100 but only 10
        // bytes of metadata + payload follow.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u64.to_le_bytes()); // global_seq
        buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
        buf.extend_from_slice(&1u16.to_le_bytes()); // event_type_len = 1
        buf.extend_from_slice(&100u32.to_le_bytes()); // meta_len = 100 (LIE)
        buf.push(b'X'); // event_type (1 byte)
        buf.extend_from_slice(&[0u8; 10]); // only 10 bytes follow, not 100

        let bytes_buf = bytes::Bytes::from(buf);
        let err = decode_event_value(&bytes_buf).expect_err("must reject");
        assert!(matches!(err, DecodeError::MetadataOutOfBounds { .. }));
    }
}
