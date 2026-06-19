use nexus_store::wire;
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

    #[error("wire-format decode error")]
    Wire(#[source] wire::DecodeError),
}

/// Errors from encoding values into byte layouts.
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("stream ID too long: {len} bytes (max {})", u16::MAX)]
    IdTooLong { len: usize },

    /// Wire-format build failure (e.g. `FrameLengthOverflow`). Wraps
    /// [`wire::WireError`] verbatim via `#[from]` so the source error's
    /// structured fields (header / padding / payload sizes) survive
    /// through the encoding-layer boundary instead of being collapsed
    /// to a bare variant.
    #[error(transparent)]
    Wire(#[from] wire::WireError),

    /// Value newtype rejected an input (oversize `event_type` / payload /
    /// metadata, invalid UTF-8 in `event_type`, empty metadata, or zero
    /// `schema_version`). The size-cap and `schema_version` invariants
    /// live on [`nexus_store::value`] now; this variant surfaces every
    /// rejection at the encoding-layer boundary.
    #[error(transparent)]
    Value(#[from] nexus_store::value::ValueError),
}

/// Size of the event key header: `[u16 BE id_len]`.
const EVENT_KEY_HEADER_SIZE: usize = 2;

/// Size of the version suffix in an event key: `[u64 BE version]`.
const EVENT_KEY_VERSION_SIZE: usize = 8;

/// Size of an encoded stream version value in bytes: `[u64 LE version]`.
pub const STREAM_VERSION_SIZE: usize = 8;

/// Size of the fixed header in an encoded event value.
///
/// Re-exported from [`wire::HEADER_FIXED_SIZE`] for adapter-local tests.
pub const EVENT_VALUE_HEADER_SIZE: usize = wire::HEADER_FIXED_SIZE;

/// Sentinel value for `meta_len` indicating no metadata.
///
/// Re-exported from [`wire::META_LEN_ABSENT`].
pub const META_LEN_ABSENT: u32 = wire::META_LEN_ABSENT;

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

/// Size of a `$all` index key: `[u64 BE global_seq][u64 BE version]`.
pub const GLOBAL_KEY_SIZE: usize = 16;

/// Encode an `events_global` key as `[u64 BE global_seq][u64 BE version]`.
///
/// `global_seq` alone is unique per event; `version` is carried so the
/// read path can reconstruct a `PersistedEnvelope` (the wire-frame value
/// does not store version — fjall keeps it in the primary event key).
/// Big-endian encoding ensures lexicographic byte order equals numeric order.
#[must_use]
pub fn encode_global_key(global_seq: u64, version: u64) -> [u8; GLOBAL_KEY_SIZE] {
    let mut buf = [0u8; GLOBAL_KEY_SIZE];
    buf[0..8].copy_from_slice(&global_seq.to_be_bytes());
    buf[8..16].copy_from_slice(&version.to_be_bytes());
    buf
}

/// Decode an `events_global` key into `(global_seq, version)`.
///
/// # Errors
///
/// Returns [`DecodeError::InvalidSize`] if `key` is not exactly [`GLOBAL_KEY_SIZE`] bytes.
pub fn decode_global_key(key: &[u8]) -> Result<(u64, u64), DecodeError> {
    if key.len() != GLOBAL_KEY_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: GLOBAL_KEY_SIZE,
            actual: key.len(),
        });
    }
    let global_seq = u64::from_be_bytes([
        key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7],
    ]);
    let version = u64::from_be_bytes([
        key[8], key[9], key[10], key[11], key[12], key[13], key[14], key[15],
    ]);
    Ok((global_seq, version))
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
/// `meta_len` is the [`META_LEN_ABSENT`] sentinel. The payload offset
/// honors the 16-byte alignment invariant from
/// [`nexus_store::wire`][wire]. `schema_version` is the typed
/// [`nexus_store::value::SchemaVersion`] — corrupt on-disk zeros are
/// surfaced as `DecodeError::Wire(wire::DecodeError::CorruptSchemaVersion)`.
#[derive(Debug)]
pub struct DecodedEvent {
    pub global_seq: u64,
    pub schema_version: nexus_store::value::SchemaVersion,
    pub event_type_range: std::ops::Range<u32>,
    pub payload_range: std::ops::Range<u32>,
    pub metadata_range: Option<std::ops::Range<u32>>,
}

/// Encode an event value via [`wire::encode_frame`] into the supplied buffer.
///
/// The buffer is cleared and overwritten with the wire-format bytes,
/// which honor the 16-byte payload alignment invariant. This wrapper
/// preserves the historical `Vec<u8>`-based API for adapter-local tests
/// and benches; the canonical builder is [`wire::encode_frame`].
///
/// # Errors
///
/// Forwards [`wire::WireError`] mapped to [`EncodeError`].
#[allow(
    clippy::too_many_arguments,
    reason = "thin wrapper preserves the historical 6-arg API for adapter-local tests/benches"
)]
pub fn encode_event_value(
    buf: &mut Vec<u8>,
    global_seq: u64,
    schema_version: u32,
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<(), EncodeError> {
    let sv = nexus_store::value::SchemaVersion::from_u32(schema_version)?;
    let et = nexus_store::value::EventType::from_bytes(bytes::Bytes::copy_from_slice(
        event_type.as_bytes(),
    ))?;
    let pl = nexus_store::value::Payload::from_bytes(bytes::Bytes::copy_from_slice(payload))?;
    let md = metadata
        .map(|m| nexus_store::value::Metadata::from_bytes(bytes::Bytes::copy_from_slice(m)))
        .transpose()?;
    let frame = wire::encode_frame(global_seq, sv, &et, &pl, md.as_ref())?;
    buf.clear();
    buf.extend_from_slice(&frame.value);
    Ok(())
}

/// Decode an event value via [`wire::decode_frame`].
///
/// Returns the existing [`DecodedEvent`] shape used by fjall's cursor code,
/// populated from the wire-format header fields and offsets. Validates that
/// the `event_type` bytes are UTF-8 (an additional check beyond
/// `wire::decode_frame`).
///
/// # Errors
///
/// - [`DecodeError::Wire`] if the wire-format decoder rejects the value.
/// - [`DecodeError::InvalidUtf8`] if `event_type` bytes are not valid UTF-8.
pub fn decode_event_value(value: &bytes::Bytes) -> Result<DecodedEvent, DecodeError> {
    let decoded = wire::decode_frame(value.as_ref()).map_err(DecodeError::Wire)?;

    // UTF-8 validation of event_type at decode (PersistedEnvelope::try_new
    // also validates; this gives early error before envelope construction).
    let et_start = usize::try_from(decoded.offsets.event_type.start).map_err(|_| {
        DecodeError::InvalidSize {
            expected: usize::MAX,
            actual: 0,
        }
    })?;
    let et_end =
        usize::try_from(decoded.offsets.event_type.end).map_err(|_| DecodeError::InvalidSize {
            expected: usize::MAX,
            actual: 0,
        })?;
    let et_slice = &value[et_start..et_end];
    std::str::from_utf8(et_slice).map_err(DecodeError::InvalidUtf8)?;

    Ok(DecodedEvent {
        global_seq: decoded.global_seq,
        schema_version: decoded.schema_version,
        event_type_range: decoded.offsets.event_type,
        payload_range: decoded.offsets.payload,
        metadata_range: decoded.offsets.metadata,
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
        assert!(key_v1 < key_v2);
    }

    #[test]
    fn event_key_length_prefix_prevents_collisions() {
        let key_foo_v1 = encode_event_key(b"foo", 1).unwrap();
        let key_foobar_v1 = encode_event_key(b"foobar", 1).unwrap();
        assert!(key_foo_v1 < key_foobar_v1);

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
        let too_short = [0u8; 1];
        assert!(decode_event_key(&too_short).is_err());
    }

    #[test]
    fn event_key_decode_rejects_wrong_total_size() {
        let mut bad = Vec::new();
        bad.extend_from_slice(&3u16.to_be_bytes());
        bad.extend_from_slice(b"foo");
        bad.extend_from_slice(&42u64.to_be_bytes()[..7]);
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
        assert_eq!(decoded.schema_version.get(), schema_version);
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
        assert_eq!(decoded.schema_version.get(), 1);
        let et = &bytes_buf
            [decoded.event_type_range.start as usize..decoded.event_type_range.end as usize];
        assert_eq!(et, b"Empty");
        assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
    }

    #[test]
    fn event_value_decode_rejects_truncated_header() {
        let short = bytes::Bytes::from(vec![0u8; 4]);
        let result = decode_event_value(&short);
        assert!(result.is_err());
    }

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
        assert_eq!(decoded.schema_version.get(), schema_version);
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
        assert_eq!(decoded.schema_version.get(), 1);
        assert!(decoded.metadata_range.is_none());
        assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
    }

    #[test]
    fn meta_len_absent_constant_is_u32_max() {
        assert_eq!(META_LEN_ABSENT, u32::MAX);
    }

    // --- Global key tests ---

    #[test]
    fn global_key_roundtrips() {
        let key = encode_global_key(42, 7);
        let (gseq, version) = decode_global_key(&key).unwrap();
        assert_eq!(gseq, 42);
        assert_eq!(version, 7);
    }

    #[test]
    fn global_keys_sort_by_global_seq_then_version() {
        // Big-endian → lexicographic byte order equals numeric order.
        let a = encode_global_key(1, 999);
        let b = encode_global_key(2, 1);
        assert!(a < b, "global_seq 1 must sort before global_seq 2");
    }

    #[test]
    fn decode_global_key_rejects_wrong_length() {
        assert!(decode_global_key(&[0u8; 8]).is_err());
        assert!(decode_global_key(&[0u8; 17]).is_err());
    }
}
