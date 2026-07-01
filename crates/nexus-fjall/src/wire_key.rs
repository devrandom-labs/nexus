use thiserror::Error;

/// Errors from decoding stored byte layouts.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },
}

/// Error from encoding a stream id into an event key.
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("stream ID too long: {len} bytes (max {})", u16::MAX)]
    IdTooLong { len: usize },
}

/// Size of the event key header: `[u16 BE id_len]`.
const EVENT_KEY_HEADER_SIZE: usize = 2;

/// Size of the version suffix in an event key: `[u64 BE version]`.
const EVENT_KEY_VERSION_SIZE: usize = 8;

/// Size of an encoded stream version value in bytes: `[u64 LE version]`.
const STREAM_VERSION_SIZE: usize = 8;

/// Compute the total size of an event key for a given ID length.
///
/// Format: `[u16 BE id_len][id_bytes][u64 BE version]`.
const fn event_key_size(id_len: usize) -> usize {
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
const GLOBAL_KEY_SIZE: usize = 16;

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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(
    clippy::panic,
    reason = "proptest macros and test assertions use panic"
)]
#[allow(
    clippy::indexing_slicing,
    reason = "test code slices fixed byte arrays"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    // --- Key-codec byte snapshots (insta) ---
    //
    // Golden `.snap` files (crates/nexus-fjall/src/snapshots/) pin the exact
    // byte layout of fjall's OWN key codecs (event key + stream-version value).
    // The event *value* format is owned and snapshot-tested by
    // `nexus_store::wire`, so it is not re-pinned here. cargo-insta is not in
    // the nix dev shell, so these are maintained by hand; when a key codec
    // changes, regenerate with `cargo insta review`.

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
}
