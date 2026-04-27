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
}

/// Errors from encoding values into byte layouts.
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("event type too long: {len} bytes (max {})", u16::MAX)]
    EventTypeTooLong { len: usize },

    #[error("stream ID too long: {len} bytes (max {})", u16::MAX)]
    IdTooLong { len: usize },
}

/// Size of the event key header: `[u16 BE id_len]`.
const EVENT_KEY_HEADER_SIZE: usize = 2;

/// Size of the version suffix in an event key: `[u64 BE version]`.
const EVENT_KEY_VERSION_SIZE: usize = 8;

/// Size of an encoded stream version value in bytes: `[u64 LE version]`.
pub const STREAM_VERSION_SIZE: usize = 8;

/// Size of the fixed header in an encoded event value: `[u32 LE schema_version][u16 LE event_type_len]`.
pub const EVENT_VALUE_HEADER_SIZE: usize = 6;

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

/// Encode an event value as `[u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]`.
///
/// The buffer is cleared and reused (no reallocation if capacity suffices).
///
/// # Errors
///
/// Returns [`EncodeError::EventTypeTooLong`] if `event_type` exceeds `u16::MAX` bytes.
pub fn encode_event_value(
    buf: &mut Vec<u8>,
    schema_version: u32,
    event_type: &str,
    payload: &[u8],
) -> Result<(), EncodeError> {
    let event_type_len = event_type.len();
    if event_type_len > usize::from(u16::MAX) {
        return Err(EncodeError::EventTypeTooLong {
            len: event_type_len,
        });
    }

    buf.clear();

    let total_len = EVENT_VALUE_HEADER_SIZE + event_type_len + payload.len();
    buf.reserve(total_len);

    buf.extend_from_slice(&schema_version.to_le_bytes());

    // Safety of unwrap alternative: we already checked event_type_len <= u16::MAX above.
    // Using ok_or + ? to satisfy the no-panic lint rules.
    let type_len_u16 =
        u16::try_from(event_type_len).map_err(|_| EncodeError::EventTypeTooLong {
            len: event_type_len,
        })?;
    buf.extend_from_slice(&type_len_u16.to_le_bytes());

    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(payload);
    Ok(())
}

/// Decode an event value from `[u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]`.
///
/// # Errors
///
/// Returns [`DecodeError::ValueTooShort`] if `value` is shorter than the header or
/// the claimed event-type length, or [`DecodeError::InvalidUtf8`] if the event-type
/// bytes are not valid UTF-8.
pub fn decode_event_value(value: &[u8]) -> Result<(u32, &str, &[u8]), DecodeError> {
    if value.len() < EVENT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: EVENT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }

    let schema_version = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let event_type_len = u16::from_le_bytes([value[4], value[5]]);
    let event_type_len_usize = usize::from(event_type_len);

    let event_type_end = EVENT_VALUE_HEADER_SIZE + event_type_len_usize;
    if value.len() < event_type_end {
        return Err(DecodeError::ValueTooShort {
            min: event_type_end,
            actual: value.len(),
        });
    }

    let event_type_bytes = &value[EVENT_VALUE_HEADER_SIZE..event_type_end];
    let event_type = std::str::from_utf8(event_type_bytes).map_err(DecodeError::InvalidUtf8)?;
    let payload = &value[event_type_end..];

    Ok((schema_version, event_type, payload))
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
        let schema_version: u32 = 3;
        let event_type = "UserCreated";
        let payload = b"some-json-bytes";

        encode_event_value(&mut buf, schema_version, event_type, payload).unwrap();
        let (decoded_sv, decoded_et, decoded_payload) = decode_event_value(&buf).unwrap();

        assert_eq!(decoded_sv, schema_version);
        assert_eq!(decoded_et, event_type);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn event_value_empty_payload() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 1, "Empty", b"").unwrap();
        let (sv, et, payload) = decode_event_value(&buf).unwrap();
        assert_eq!(sv, 1);
        assert_eq!(et, "Empty");
        assert!(payload.is_empty());
    }

    #[test]
    fn event_value_decode_rejects_truncated_header() {
        let short = [0u8; 4]; // less than EVENT_VALUE_HEADER_SIZE (6)
        let result = decode_event_value(&short);
        assert!(result.is_err());
    }

    #[test]
    fn event_value_decode_rejects_truncated_event_type() {
        // Build a header that claims event_type_len = 100 but provide no type bytes
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
        buf.extend_from_slice(&100u16.to_le_bytes()); // event_type_len = 100
        // No event_type bytes follow
        let result = decode_event_value(&buf);
        assert!(result.is_err());
    }

    #[test]
    fn event_value_reuses_buffer() {
        let mut buf = Vec::new();

        // First encode
        encode_event_value(&mut buf, 1, "First", b"aaa").unwrap();
        let cap_after_first = buf.capacity();

        // Second encode reuses the buffer (clear, no realloc if capacity suffices)
        encode_event_value(&mut buf, 2, "Sec", b"bb").unwrap();
        assert_eq!(buf.capacity(), cap_after_first);

        // Verify the second encode is correct (not contaminated by first)
        let (sv, et, payload) = decode_event_value(&buf).unwrap();
        assert_eq!(sv, 2);
        assert_eq!(et, "Sec");
        assert_eq!(payload, b"bb");
    }
}
