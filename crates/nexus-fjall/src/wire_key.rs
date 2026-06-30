use nexus_store::wire;
use thiserror::Error;

/// Errors from decoding stored byte layouts.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },

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
/// `meta_len` is the absent sentinel (`u32::MAX`). The payload offset
/// honors the 16-byte alignment invariant from
/// [`nexus_store::wire`][wire]. `schema_version` is the typed
/// [`nexus_store::value::SchemaVersion`] — corrupt on-disk zeros are
/// surfaced as `DecodeError::Wire(wire::DecodeError::CorruptSchemaVersion)`.
#[derive(Debug)]
pub struct DecodedEvent {
    pub schema_version: nexus_store::value::SchemaVersion,
    pub event_type_range: std::ops::Range<u32>,
    pub payload_range: std::ops::Range<u32>,
    pub metadata_range: Option<std::ops::Range<u32>>,
}

/// Decode an event value via [`wire::decode_frame`].
///
/// Returns the existing [`DecodedEvent`] shape used by fjall's cursor code,
/// populated from the wire-format header fields and offsets. The
/// `event_type` bytes are **not** UTF-8-validated here: the read path's
/// [`PersistedEnvelope::try_new`][nexus_store::PersistedEnvelope::try_new]
/// validates them downstream and surfaces a non-UTF-8 `event_type` as
/// `EnvelopeError::InvalidUtf8` (mapped to `FjallError::EnvelopeCorrupt`),
/// so a redundant pre-check here would only duplicate that guarantee.
///
/// # Errors
///
/// - [`DecodeError::Wire`] if the wire-format decoder rejects the value.
pub fn decode_event_value(value: &bytes::Bytes) -> Result<DecodedEvent, DecodeError> {
    let decoded = wire::decode_frame(value.as_ref()).map_err(DecodeError::Wire)?;

    Ok(DecodedEvent {
        schema_version: decoded.schema_version,
        event_type_range: decoded.offsets.event_type,
        payload_range: decoded.offsets.payload,
        metadata_range: decoded.offsets.metadata,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::expect_used, reason = "test code")]
#[allow(
    clippy::panic,
    reason = "proptest macros and test assertions use panic"
)]
#[allow(clippy::print_stdout, reason = "diagnostic output in evil-input tests")]
#[allow(clippy::indexing_slicing, reason = "test code")]
#[allow(clippy::missing_panics_doc, reason = "test helpers")]
#[allow(clippy::shadow_reuse, reason = "test code reuses local bindings")]
#[allow(clippy::shadow_unrelated, reason = "test code reuses local bindings")]
#[allow(
    clippy::as_conversions,
    reason = "u32→usize casts in tests are safe on 32-bit+ platforms"
)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use proptest::prelude::*;

    /// Build a wire-frame event-value row via the REAL production encoder
    /// ([`wire::encode_frame`] + the [`nexus_store::value`] newtypes) for
    /// in-crate white-box codec tests: it exercises the exact byte layout the
    /// read path (`decode_event_value`) consumes, surfacing the same
    /// [`EncodeError`] mapping the production append path would.
    ///
    /// # Errors
    ///
    /// Forwards [`wire::WireError`] / [`nexus_store::value::ValueError`] mapped
    /// to [`EncodeError`] (e.g. `schema_version == 0`, oversize `event_type`).
    fn test_row(
        schema_version: u32,
        event_type: &str,
        metadata: Option<&[u8]>,
        payload: &[u8],
    ) -> Result<Vec<u8>, EncodeError> {
        let sv = nexus_store::value::SchemaVersion::from_u32(schema_version)?;
        let et = nexus_store::value::EventType::from_bytes(Bytes::copy_from_slice(
            event_type.as_bytes(),
        ))?;
        let pl = nexus_store::value::Payload::from_bytes(Bytes::copy_from_slice(payload))?;
        let md = metadata
            .map(|m| nexus_store::value::Metadata::from_bytes(Bytes::copy_from_slice(m)))
            .transpose()?;
        let frame = wire::encode_frame(sv, &et, &pl, md.as_ref())?;
        Ok(frame.value.to_vec())
    }

    /// Decode an event value from a `Vec<u8>` buffer and extract all fields as
    /// concrete values: `(schema_version, event_type, payload)`. The `$all`
    /// position is not in the value (V2) — it is the `events_global` key.
    fn decode_ev_slices(buf: &[u8]) -> (u32, String, Vec<u8>) {
        let b = Bytes::copy_from_slice(buf);
        let d = decode_event_value(&b).unwrap();
        let et = std::str::from_utf8(
            &b[d.event_type_range.start as usize..d.event_type_range.end as usize],
        )
        .unwrap()
        .to_owned();
        let pl = b[d.payload_range.start as usize..d.payload_range.end as usize].to_vec();
        (d.schema_version.get(), et, pl)
    }

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
        let schema_version: u32 = 3;
        let event_type = "UserCreated";
        let payload = b"some-json-bytes";

        let buf = test_row(schema_version, event_type, None, payload).unwrap();
        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

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
        let buf = test_row(1, "Empty", None, b"").unwrap();
        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();
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
        let schema_version: u32 = 3;
        let event_type = "UserCreated";
        let metadata = b"correlation-abc-123".as_slice();
        let payload = b"some-json-bytes";

        let buf = test_row(schema_version, event_type, Some(metadata), payload).unwrap();

        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

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
        let buf = test_row(1, "Empty", None, b"").unwrap();

        let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
        let decoded = decode_event_value(&bytes_buf).unwrap();

        assert_eq!(decoded.schema_version.get(), 1);
        assert!(decoded.metadata_range.is_none());
        assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
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

    // ─────────────────────────────────────────────────────────────────────
    // Relocated white-box codec tests (formerly in tests/{property,resilience,
    // metadata_roundtrip}.rs — they reach the now-private codec, so they live
    // in-crate). Public-API exercising tests stayed as integration tests.
    // ─────────────────────────────────────────────────────────────────────

    // --- Defensive boundary: decoder rejects corrupt meta_len ---
    // (relocated from tests/metadata_roundtrip_tests.rs)

    #[test]
    fn decoder_rejects_meta_len_exceeding_buffer() {
        // Manually build a V2 value that *claims* meta_len = 100 but only 10
        // bytes of metadata/payload follow. The decoder must reject rather than
        // read past the buffer end.
        let mut buf = Vec::new();
        buf.push(2u8); // frame_format_version = 2 (V2)
        buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
        buf.extend_from_slice(&1u16.to_le_bytes()); // event_type_len = 1
        buf.extend_from_slice(&100u32.to_le_bytes()); // meta_len = 100 (LIE)
        buf.push(b'X'); // event_type (1 byte)
        buf.extend_from_slice(&[0u8; 10]); // only 10 bytes follow — not 100

        let bytes_buf = Bytes::from(buf);
        let err = decode_event_value(&bytes_buf).expect_err("must reject");
        assert!(
            matches!(
                err,
                DecodeError::Wire(wire::DecodeError::MetadataTruncated { meta_len: 100, .. })
            ),
            "expected Wire(MetadataTruncated {{ meta_len: 100, .. }}), got {err:?}",
        );
    }

    // --- Encoding attack surface (proptest) ---
    // (relocated from tests/property_tests.rs CATEGORY 1)

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// For any (id_bytes, version) pair, encode_event_key -> decode_event_key is identity.
        #[test]
        fn attack_encoding_event_key_round_trip_any_values(
            id_bytes in prop::collection::vec(any::<u8>(), 0..200),
            version in any::<u64>(),
        ) {
            let encoded = encode_event_key(&id_bytes, version).unwrap();
            let (decoded_id, decoded_version) = decode_event_key(&encoded).unwrap();
            prop_assert_eq!(decoded_id, id_bytes.as_slice(), "id_bytes round-trip failed");
            prop_assert_eq!(decoded_version, version, "version round-trip failed");
        }

        /// For any u64, encode_stream_version -> decode_stream_version is identity.
        #[test]
        fn attack_encoding_stream_version_round_trip_any_values(
            version in any::<u64>(),
        ) {
            let encoded = encode_stream_version(version);
            let decoded = decode_stream_version(&encoded).unwrap();
            prop_assert_eq!(decoded, version, "version round-trip failed");
        }

        /// For any (u64, u32, String, Vec<u8>) tuple where event_type.len() <= u16::MAX,
        /// test_row -> decode_event_value is identity.
        #[test]
        fn attack_encoding_event_value_round_trip_any_data(
            schema_ver in 1..=u32::MAX,
            event_type in "[a-zA-Z_][a-zA-Z0-9_]{0,200}",
            payload in prop::collection::vec(any::<u8>(), 0..1024),
        ) {
            let buf = test_row(schema_ver, &event_type, None, &payload).unwrap();
            let (dec_sv, dec_et, dec_payload) = decode_ev_slices(&buf);
            prop_assert_eq!(dec_sv, schema_ver, "schema_version round-trip failed");
            prop_assert_eq!(dec_et, event_type, "event_type round-trip failed");
            prop_assert_eq!(dec_payload, payload.as_slice(), "payload round-trip failed");
        }

        /// For any a < b (u64), encode_event_key(id, a) < encode_event_key(id, b).
        #[test]
        fn attack_encoding_event_key_byte_ordering(
            id_bytes in prop::collection::vec(any::<u8>(), 0..50),
            a in 0..u64::MAX,
        ) {
            let b = a + 1;
            let key_a = encode_event_key(&id_bytes, a).unwrap();
            let key_b = encode_event_key(&id_bytes, b).unwrap();
            prop_assert!(key_a < key_b,
                "version ordering violated for same ID");
        }

        /// For IDs with different length prefixes, shorter ID sorts before longer.
        #[test]
        fn attack_encoding_event_key_length_prefix_ordering(
            base in prop::collection::vec(any::<u8>(), 1..50),
            extra in prop::collection::vec(any::<u8>(), 1..10),
            version in any::<u64>(),
        ) {
            let mut longer = base.clone();
            longer.extend_from_slice(&extra);
            let key_short = encode_event_key(&base, version).unwrap();
            let key_long = encode_event_key(&longer, version).unwrap();
            // Shorter length prefix (u16 BE) sorts before longer
            prop_assert!(key_short < key_long,
                "length prefix ordering violated: shorter ID must sort before longer");
        }
    }

    /// Evil event type strings — the encoding must handle them or reject them cleanly.
    #[test]
    fn attack_encoding_event_value_evil_event_types() {
        let evil_types: Vec<(&str, &str)> = vec![
            ("", "empty string"),
            ("日本語🔥", "Unicode with emoji"),
            ("\0\0\0", "null bytes"),
            ("   \t\n  ", "whitespace only"),
            ("../../../etc/passwd", "path traversal"),
            ("'; DROP TABLE events; --", "SQL injection"),
            ("\u{200B}", "zero-width space"),
            ("\u{FEFF}BOM", "byte order mark prefix"),
        ];

        for (evil_type, description) in &evil_types {
            match test_row(1, evil_type, None, b"test") {
                Ok(buf) => {
                    let (sv, decoded_type, payload) = decode_ev_slices(&buf);
                    assert_eq!(sv, 1, "schema_version corrupted for: {description}");
                    assert_eq!(
                        decoded_type, *evil_type,
                        "event_type corrupted for: {description}",
                    );
                    assert_eq!(payload, b"test", "payload corrupted for: {description}");
                }
                Err(_) => {
                    // Rejection is also acceptable — document it.
                    println!("Encoding rejected evil event type '{evil_type}' ({description})");
                }
            }
        }

        // Very long string near u16::MAX bytes
        let long_type = "a".repeat(usize::from(u16::MAX));
        let buf = test_row(1, &long_type, None, b"x")
            .expect("event_type at exactly u16::MAX bytes must be accepted");
        let (_, decoded, _) = decode_ev_slices(&buf);
        assert_eq!(decoded.len(), usize::from(u16::MAX));

        // One byte over u16::MAX must be rejected
        let too_long_type = "a".repeat(usize::from(u16::MAX) + 1);
        let result = test_row(1, &too_long_type, None, b"x");
        assert!(
            result.is_err(),
            "event_type exceeding u16::MAX bytes must be rejected"
        );
    }

    // --- Encoding boundary attacks ---
    // (relocated from tests/resilience_tests.rs CATEGORY I)

    #[test]
    fn attack_encoding_event_type_exactly_u16_max_bytes() {
        let event_type = "a".repeat(usize::from(u16::MAX));
        let buf = test_row(1, &event_type, None, b"payload")
            .expect("event type at exactly u16::MAX bytes should succeed");

        let (sv, decoded_type, payload) = decode_ev_slices(&buf);
        assert_eq!(sv, 1);
        assert_eq!(decoded_type.len(), usize::from(u16::MAX));
        assert_eq!(payload, b"payload");
    }

    #[test]
    fn attack_encoding_event_type_one_over_u16_max() {
        let event_type = "a".repeat(usize::from(u16::MAX) + 1);
        let result = test_row(1, &event_type, None, b"payload");
        assert!(
            result.is_err(),
            "event type over u16::MAX must fail with EncodeError"
        );
    }

    #[test]
    fn attack_encoding_empty_event_type() {
        let buf = test_row(1, "", None, b"payload").expect("empty event type should encode");

        let (sv, decoded_type, payload) = decode_ev_slices(&buf);
        assert_eq!(sv, 1);
        assert_eq!(decoded_type, "");
        assert_eq!(payload, b"payload");
    }

    #[test]
    fn attack_encoding_empty_payload() {
        let buf = test_row(1, "Test", None, b"").expect("empty payload should encode");

        let (sv, decoded_type, payload) = decode_ev_slices(&buf);
        assert_eq!(sv, 1);
        assert_eq!(decoded_type, "Test");
        assert!(payload.is_empty());
    }

    #[test]
    fn attack_encoding_null_bytes_in_payload() {
        let evil_payload = b"\x00\x00\x00\x00\x00";
        let buf = test_row(1, "Test", None, evil_payload).unwrap();
        let (_, _, payload) = decode_ev_slices(&buf);
        assert_eq!(payload, evil_payload, "null bytes in payload must survive");
    }

    #[test]
    fn attack_encoding_schema_version_boundaries() {
        // schema_version = 0 must be rejected at the encoding layer — the
        // wire builder ([`wire::encode_frame`]) and `PersistedEnvelope::try_new`
        // both reject 0, restoring write/read symmetry (CLAUDE.md §3, §4).
        let result = test_row(0, "Test", None, b"data");
        match result {
            Err(EncodeError::Value(nexus_store::value::ValueError::SchemaVersionZero)) => {}
            other => panic!("expected Value(SchemaVersionZero) at encoding layer, got {other:?}"),
        }

        // schema_version = 1 is the minimum valid value and must round-trip.
        let buf = test_row(1, "Test", None, b"data").unwrap();
        let (sv, _, _) = decode_ev_slices(&buf);
        assert_eq!(sv, 1, "schema_version=1 must round-trip");

        // schema_version = u32::MAX is the upper boundary and must round-trip.
        let buf = test_row(u32::MAX, "Test", None, b"data").unwrap();
        let (sv, _, _) = decode_ev_slices(&buf);
        assert_eq!(
            sv,
            u32::MAX,
            "schema_version=u32::MAX must round-trip in encoding layer"
        );
    }

    #[test]
    fn attack_encoding_large_payload() {
        // 1MB payload
        let large = vec![0xABu8; 1_048_576];
        let buf = test_row(1, "Big", None, &large).unwrap();
        let (_, _, payload) = decode_ev_slices(&buf);
        assert_eq!(
            payload.len(),
            1_048_576,
            "1MB payload must survive encoding"
        );
        assert_eq!(payload[0], 0xAB);
        assert_eq!(payload[1_048_575], 0xAB);
    }

    // --- Wire-format byte snapshots (insta) ---
    // (relocated from tests/snapshot_wire_format.rs)
    //
    // The golden `.snap` files (crates/nexus-fjall/src/snapshots/) were
    // hand-verified against the wire-format spec. cargo-insta is not in the nix
    // dev shell, so they are maintained by hand; when the wire format changes,
    // regenerate them with `cargo insta review`.

    fn hex_dump(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn event_key_empty_id_v0() {
        let encoded = encode_event_key(b"", 0).unwrap();
        insta::assert_snapshot!("event_key_empty_id_v0", hex_dump(&encoded));
    }

    #[test]
    fn event_key_a_v1() {
        let encoded = encode_event_key(b"a", 1).unwrap();
        insta::assert_snapshot!("event_key_a_v1", hex_dump(&encoded));
    }

    #[test]
    fn event_key_stream42_v7() {
        let encoded = encode_event_key(b"stream-42", 7).unwrap();
        insta::assert_snapshot!("event_key_stream42_v7", hex_dump(&encoded));
    }

    #[test]
    fn event_key_empty_id_vmax() {
        let encoded = encode_event_key(b"", u64::MAX).unwrap();
        insta::assert_snapshot!("event_key_empty_id_vmax", hex_dump(&encoded));
    }

    #[test]
    fn event_key_ordering_version() {
        let key_v1 = encode_event_key(b"s1", 1).unwrap();
        let key_v2 = encode_event_key(b"s1", 2).unwrap();
        insta::assert_snapshot!("event_key_s1_v1", hex_dump(&key_v1));
        insta::assert_snapshot!("event_key_s1_v2", hex_dump(&key_v2));
        assert!(
            key_v1 < key_v2,
            "version 1 must sort before version 2 in byte order"
        );
    }

    #[test]
    fn event_key_stream_grouping() {
        let key_foo_v100 = encode_event_key(b"foo", 100).unwrap();
        let key_foobar_v1 = encode_event_key(b"foobar", 1).unwrap();
        insta::assert_snapshot!("event_key_foo_v100", hex_dump(&key_foo_v100));
        insta::assert_snapshot!("event_key_foobar_v1", hex_dump(&key_foobar_v1));
        // Length prefix ensures "foo" range doesn't overlap "foobar"
        assert!(
            key_foo_v100 < key_foobar_v1,
            "shorter ID must sort before longer ID with same prefix"
        );
    }

    #[test]
    fn stream_version_zero() {
        let encoded = encode_stream_version(0);
        insta::assert_snapshot!("stream_version_zero", hex_dump(&encoded));
    }

    #[test]
    fn stream_version_one() {
        let encoded = encode_stream_version(1);
        insta::assert_snapshot!("stream_version_one", hex_dump(&encoded));
    }

    #[test]
    fn stream_version_999() {
        let encoded = encode_stream_version(999);
        insta::assert_snapshot!("stream_version_999", hex_dump(&encoded));
    }

    #[test]
    fn stream_version_max() {
        let encoded = encode_stream_version(u64::MAX);
        insta::assert_snapshot!("stream_version_max", hex_dump(&encoded));
    }

    #[test]
    fn event_value_typical() {
        let buf = test_row(1, "UserCreated", None, b"payload").unwrap();
        insta::assert_snapshot!("event_value_typical", hex_dump(&buf));
    }

    #[test]
    fn event_value_schema_zero_rejected() {
        // schema_version=0 is now rejected at the encoding layer to stay
        // symmetric with `PersistedEnvelope::try_new` (CLAUDE.md §3, §4).
        let result = test_row(0, "Empty", None, b"");
        assert!(
            matches!(
                result,
                Err(EncodeError::Value(
                    nexus_store::value::ValueError::SchemaVersionZero
                ))
            ),
            "test_row must reject schema_version=0, got {result:?}",
        );
    }

    #[test]
    fn event_value_schema_min_empty_payload() {
        // The pair we want to cover is "minimum valid schema_version + empty
        // payload" — schema_version=1 is the minimum now that 0 is rejected.
        let buf = test_row(1, "Empty", None, b"").unwrap();
        insta::assert_snapshot!("event_value_schema_min_empty_payload", hex_dump(&buf));
    }

    #[test]
    fn event_value_max_schema_binary_payload() {
        let buf = test_row(u32::MAX, "X", None, b"\x00\xff").unwrap();
        insta::assert_snapshot!("event_value_max_schema_binary_payload", hex_dump(&buf));
    }

    #[test]
    fn event_value_empty_event_type() {
        let buf = test_row(1, "", None, b"data").unwrap();
        insta::assert_snapshot!("event_value_empty_event_type", hex_dump(&buf));
    }

    #[test]
    fn event_value_single_char_type_empty_payload() {
        let buf = test_row(1, "A", None, b"").unwrap();
        insta::assert_snapshot!("event_value_single_char_type_empty_payload", hex_dump(&buf));
    }

    #[test]
    fn event_value_larger_payload() {
        let buf = test_row(42, "OrderPlaced", None, &[0u8; 1024]).unwrap();
        insta::assert_snapshot!("event_value_larger_payload", hex_dump(&buf));
    }
}
