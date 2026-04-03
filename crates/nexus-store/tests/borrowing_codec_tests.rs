//! Tests for `BorrowingCodec` trait.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus_store::BorrowingCodec;

/// A test codec that "decodes" by reinterpreting bytes as a u32 slice.
struct U32Codec;

impl BorrowingCodec<[u32]> for U32Codec {
    type Error = std::io::Error;

    fn encode(&self, event: &[u32]) -> Result<Vec<u8>, Self::Error> {
        Ok(event.iter().flat_map(|n| n.to_le_bytes()).collect())
    }

    fn decode<'a>(&self, _event_type: &str, payload: &'a [u8]) -> Result<&'a [u32], Self::Error> {
        if !payload.len().is_multiple_of(4) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "payload length not multiple of 4",
            ));
        }
        let (prefix, shorts, suffix) = unsafe { payload.align_to::<u32>() };
        if !prefix.is_empty() || !suffix.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unaligned",
            ));
        }
        Ok(shorts)
    }
}

#[test]
fn borrowing_codec_decode_borrows_from_payload() {
    let codec = U32Codec;
    let original: Vec<u32> = vec![1, 2, 3];
    let encoded = codec.encode(&original).unwrap();
    let decoded = codec.decode("event", &encoded).unwrap();
    assert_eq!(decoded, &[1, 2, 3]);
}

#[test]
fn borrowing_codec_decode_rejects_bad_payload() {
    let codec = U32Codec;
    let bad = vec![1, 2, 3];
    assert!(codec.decode("event", &bad).is_err());
}

#[test]
fn borrowing_codec_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<U32Codec>();
}
