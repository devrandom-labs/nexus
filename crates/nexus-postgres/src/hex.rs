//! Tiny hex codec for the `LISTEN/NOTIFY` payload.
//!
//! A `NOTIFY` payload is a text string, but a stream id is arbitrary bytes
//! (`StreamKey` is `BYTEA`), so the write side ([`notify_committed`]) hex-encodes
//! the raw id and the listener task ([`decode`]) reverses it to recover the exact
//! bytes to hand to `StreamNotifiers::wake`. Extracted here so both sides share
//! one definition and the round-trip is unit-testable without a database.
//!
//! No `hex` crate dependency — the operation is two nibbles per byte, so an
//! inline codec is smaller than the dependency edge it would add.
//!
//! [`notify_committed`]: crate::store::PostgresStore::notify_committed

/// Lowercase hex alphabet, indexed by nibble.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Hex-encode `bytes` into a lowercase ASCII string (two chars per byte).
pub fn encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|b| {
            [
                char::from(HEX[usize::from(*b >> 4)]),
                char::from(HEX[usize::from(*b & 0x0f)]),
            ]
        })
        .collect()
}

/// Decode one lowercase-or-uppercase hex digit to its 0..=15 value.
const fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decode a hex string back to bytes.
///
/// Returns `None` for an odd length or any non-hex character — a malformed
/// payload is dropped rather than mis-routing a wake to the wrong stream. This
/// is safe: a dropped wake merely delays the next scan (see the listener task's
/// correctness comment), it never loses an event.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| Some((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::{decode, encode};

    /// Round-trip: every byte value survives encode → decode unchanged.
    #[test]
    fn round_trips_all_byte_values() {
        let all: Vec<u8> = (0..=255u16).map(|n| u8::try_from(n).unwrap()).collect();
        let encoded = encode(&all);
        assert_eq!(encoded.len(), all.len() * 2, "two hex chars per byte");
        assert_eq!(
            decode(&encoded).unwrap(),
            all,
            "encode then decode is identity"
        );
    }

    /// The empty stream id round-trips to the empty payload and back.
    #[test]
    fn empty_round_trips() {
        assert_eq!(encode(&[]), "");
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }

    /// Encoding is lowercase and matches the known vector for `0xDE 0xAD`.
    #[test]
    fn encodes_known_vector_lowercase() {
        assert_eq!(encode(&[0xde, 0xad]), "dead");
    }

    /// Decode accepts uppercase hex too (defensive — a manual `NOTIFY` might use
    /// it), producing the same bytes as lowercase.
    #[test]
    fn decode_accepts_uppercase() {
        assert_eq!(decode("DEAD").unwrap(), vec![0xde, 0xad]);
    }

    /// An odd-length payload is rejected, not silently truncated.
    #[test]
    fn odd_length_is_rejected() {
        assert_eq!(decode("abc"), None);
    }

    /// A non-hex character is rejected rather than mis-decoded.
    #[test]
    fn non_hex_char_is_rejected() {
        assert_eq!(decode("zz"), None);
        assert_eq!(decode("0g"), None);
    }
}
