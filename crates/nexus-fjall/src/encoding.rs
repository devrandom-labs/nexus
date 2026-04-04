use thiserror::Error;

/// Errors from decoding stored byte layouts.
#[derive(Debug, Error)]
pub(crate) enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },

    #[error("invalid UTF-8 in event type")]
    InvalidUtf8(#[source] std::str::Utf8Error),
}

/// Errors from encoding values into byte layouts.
#[derive(Debug, Error)]
#[allow(dead_code, reason = "used by RawEventStore::append (Task 6)")]
pub(crate) enum EncodeError {
    #[error("event type too long: {len} bytes (max {})", u16::MAX)]
    EventTypeTooLong { len: usize },
}

/// Size of an encoded event key in bytes: `[u64 BE stream_numeric_id][u64 BE version]`.
pub(crate) const EVENT_KEY_SIZE: usize = 16;

/// Size of an encoded stream metadata value in bytes: `[u64 LE numeric_id][u64 LE version]`.
pub(crate) const STREAM_META_SIZE: usize = 16;

/// Size of the fixed header in an encoded event value: `[u32 LE schema_version][u16 LE event_type_len]`.
pub(crate) const EVENT_VALUE_HEADER_SIZE: usize = 6;

/// Encode an event key as `[u64 BE stream_numeric_id][u64 BE version]`.
///
/// Big-endian encoding ensures natural byte ordering in LSM trees.
#[allow(dead_code, reason = "used by RawEventStore::append (Task 6)")]
pub(crate) fn encode_event_key(stream_numeric_id: u64, version: u64) -> [u8; EVENT_KEY_SIZE] {
    let mut buf = [0u8; EVENT_KEY_SIZE];
    buf[..8].copy_from_slice(&stream_numeric_id.to_be_bytes());
    buf[8..].copy_from_slice(&version.to_be_bytes());
    buf
}

/// Decode an event key from `[u64 BE stream_numeric_id][u64 BE version]`.
pub(crate) fn decode_event_key(key: &[u8]) -> Result<(u64, u64), DecodeError> {
    if key.len() != EVENT_KEY_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: EVENT_KEY_SIZE,
            actual: key.len(),
        });
    }

    let stream_numeric_id = u64::from_be_bytes([
        key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7],
    ]);
    let version = u64::from_be_bytes([
        key[8], key[9], key[10], key[11], key[12], key[13], key[14], key[15],
    ]);

    Ok((stream_numeric_id, version))
}

/// Encode stream metadata as `[u64 LE numeric_id][u64 LE version]`.
///
/// Little-endian encoding is used since metadata has no ordering requirement.
#[allow(dead_code, reason = "used by RawEventStore::append (Task 6)")]
pub(crate) fn encode_stream_meta(numeric_id: u64, version: u64) -> [u8; STREAM_META_SIZE] {
    let mut buf = [0u8; STREAM_META_SIZE];
    buf[..8].copy_from_slice(&numeric_id.to_le_bytes());
    buf[8..].copy_from_slice(&version.to_le_bytes());
    buf
}

/// Decode stream metadata from `[u64 LE numeric_id][u64 LE version]`.
pub(crate) fn decode_stream_meta(value: &[u8]) -> Result<(u64, u64), DecodeError> {
    if value.len() != STREAM_META_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: STREAM_META_SIZE,
            actual: value.len(),
        });
    }

    let numeric_id = u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]);
    let version = u64::from_le_bytes([
        value[8], value[9], value[10], value[11], value[12], value[13], value[14], value[15],
    ]);

    Ok((numeric_id, version))
}

/// Encode an event value as `[u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]`.
///
/// The buffer is cleared and reused (no reallocation if capacity suffices).
/// Returns an error if `event_type` exceeds `u16::MAX` bytes.
#[allow(dead_code, reason = "used by RawEventStore::append (Task 6)")]
pub(crate) fn encode_event_value(
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
pub(crate) fn decode_event_value(value: &[u8]) -> Result<(u32, &str, &[u8]), DecodeError> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    // --- Event key tests ---

    #[test]
    fn event_key_round_trips() {
        let stream_id: u64 = 42;
        let version: u64 = 7;
        let encoded = encode_event_key(stream_id, version);
        let (decoded_stream, decoded_version) = decode_event_key(&encoded).unwrap();
        assert_eq!(decoded_stream, stream_id);
        assert_eq!(decoded_version, version);
    }

    #[test]
    fn event_key_is_big_endian_for_ordering() {
        let key_v1 = encode_event_key(1, 1);
        let key_v2 = encode_event_key(1, 2);
        // Byte-level comparison: version 1 < version 2
        assert!(key_v1 < key_v2);
    }

    #[test]
    fn event_key_groups_by_stream() {
        // stream 1, version 100 should sort before stream 2, version 1
        let key_s1_v100 = encode_event_key(1, 100);
        let key_s2_v1 = encode_event_key(2, 1);
        assert!(key_s1_v100 < key_s2_v1);
    }

    #[test]
    fn event_key_decode_rejects_wrong_size() {
        let too_short = [0u8; 15];
        let short_result = decode_event_key(&too_short);
        assert!(short_result.is_err());

        let too_long = [0u8; 17];
        let long_result = decode_event_key(&too_long);
        assert!(long_result.is_err());
    }

    // --- Stream meta tests ---

    #[test]
    fn stream_meta_round_trips() {
        let numeric_id: u64 = 999;
        let version: u64 = 5;
        let encoded = encode_stream_meta(numeric_id, version);
        let (decoded_id, decoded_version) = decode_stream_meta(&encoded).unwrap();
        assert_eq!(decoded_id, numeric_id);
        assert_eq!(decoded_version, version);
    }

    #[test]
    fn stream_meta_decode_rejects_wrong_size() {
        let too_short = [0u8; 8];
        let short_result = decode_stream_meta(&too_short);
        assert!(short_result.is_err());

        let too_long = [0u8; 24];
        let long_result = decode_stream_meta(&too_long);
        assert!(long_result.is_err());
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
