//! The default CBOR backup box — the bytes↔sections travel codec.
//!
//! Sits between Card 1's raw export (per-stream [`PersistedEnvelope`]s) and
//! Card 2's [`EventImporter`](crate::import::EventImporter). A chunk is a CBOR
//! sequence (RFC 8742): a header map, then per-stream section headings each
//! followed by block arrays `[crc32c, body]`. See
//! `docs/plans/2026-06-21-export-import-cbor-box-design.md`.

use core::convert::Infallible;

use bytes::Bytes;
use thiserror::Error;

use crate::envelope::PersistedEnvelope;
use crate::import::StreamSection;

const MAGIC: &[u8] = b"nxch";
const FORMAT_VERSION: u32 = 1;

/// A box-layer encode/decode failure.
///
/// Not [`ImportError`](crate::import::ImportError) — the box has no store error
/// or id type. Two failure domains, two variants (CLAUDE rule 3).
#[derive(Debug, Error)]
pub enum ChunkError {
    /// Decode: the chunk framing is unreadable. The `&'static str` is a debug
    /// hint. Distinct from a per-block crc failure, which is a non-error
    /// [`ImportBlock::Corrupt`](crate::import::ImportBlock::Corrupt).
    #[error("malformed chunk: {0}")]
    Malformed(&'static str),
    /// Encode: minicbor serialization failed. Never fires in practice (the
    /// `Vec` writer is `Infallible`, a `PersistedEnvelope` guarantees every
    /// field cap) but minicbor's API is fallible, so the type is honest.
    #[error("chunk encode failed: {0}")]
    Encode(#[from] minicbor::encode::Error<Infallible>),
}

/// The decoded chunk header — what [`decode_header`] returns.
#[derive(Debug, Clone)]
pub struct ChunkHeader {
    /// The chunk format version (always `1` for a chunk this build accepts).
    pub format_version: u32,
    /// The chunk-level producer/device id, if the encoder recorded one.
    pub origin: Option<Bytes>,
}

/// Wire shape of the header map `{0: magic, 1: format_version, 2: origin?}`.
#[derive(minicbor::Encode, minicbor::Decode)]
#[cbor(map)]
struct HeaderRepr<'a> {
    #[n(0)]
    #[cbor(with = "minicbor::bytes")]
    magic: &'a [u8],
    #[n(1)]
    format_version: u32,
    #[n(2)]
    #[cbor(with = "minicbor::bytes")]
    origin: Option<&'a [u8]>,
}

fn validate_header(magic: &[u8], format_version: u32) -> Result<(), ChunkError> {
    if magic != MAGIC {
        return Err(ChunkError::Malformed("bad magic"));
    }
    if format_version != FORMAT_VERSION {
        return Err(ChunkError::Malformed("unknown format version"));
    }
    Ok(())
}

/// Encode the chunk header. Call once, at the start of a chunk. `origin` is an
/// optional chunk-level producer/device id (omitted from the map when `None`).
///
/// # Errors
/// Returns [`ChunkError::Encode`] if minicbor serialization fails (never in
/// practice — the `Vec` writer is infallible).
pub fn encode_header(origin: Option<&[u8]>) -> Result<Bytes, ChunkError> {
    let repr = HeaderRepr {
        magic: MAGIC,
        format_version: FORMAT_VERSION,
        origin,
    };
    Ok(Bytes::from(minicbor::to_vec(&repr)?))
}

/// Decode just the header — a cheap peek that validates magic + format version
/// and returns the chunk-level origin without parsing the body.
///
/// # Errors
/// Returns [`ChunkError::Malformed`] on bad magic, unknown format version, or
/// unreadable header bytes.
pub fn decode_header(bytes: &[u8]) -> Result<ChunkHeader, ChunkError> {
    let repr: HeaderRepr =
        minicbor::decode(bytes).map_err(|_| ChunkError::Malformed("unreadable header"))?;
    validate_header(repr.magic, repr.format_version)?;
    Ok(ChunkHeader {
        format_version: repr.format_version,
        origin: repr.origin.map(Bytes::copy_from_slice),
    })
}

/// Stub — replaced in Task 3.
///
/// # Errors
///
/// Returns [`ChunkError::Encode`] if the CBOR serialization fails (infallible
/// in practice for the stub path).
pub const fn encode_section_heading(_stream_id: &[u8]) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}

/// Stub — replaced in Task 3.
///
/// # Errors
///
/// Returns [`ChunkError::Encode`] if the CBOR serialization fails (infallible
/// in practice for the stub path).
pub const fn encode_block(_event: &PersistedEnvelope) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}

/// Stub — replaced in Task 4.
///
/// # Errors
///
/// Returns [`ChunkError::Malformed`] if the chunk bytes cannot be decoded.
pub const fn decode_chunk(_bytes: &[u8]) -> Result<Vec<StreamSection>, ChunkError> {
    Ok(Vec::new())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code asserts exact values"
)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips_without_origin() {
        let bytes = encode_header(None).expect("encode");
        let header = decode_header(&bytes).expect("decode");
        assert_eq!(header.format_version, 1);
        assert_eq!(header.origin, None);
    }

    #[test]
    fn header_round_trips_with_origin() {
        let bytes = encode_header(Some(b"phone-7")).expect("encode");
        let header = decode_header(&bytes).expect("decode");
        assert_eq!(header.format_version, 1);
        assert_eq!(header.origin.as_deref(), Some(b"phone-7".as_slice()));
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut v = encode_header(None).expect("encode").to_vec();
        let pos = v.iter().position(|&b| b == b'n').expect("magic present");
        v[pos] = b'X';
        let err = decode_header(&v).expect_err("bad magic rejected");
        assert!(matches!(err, ChunkError::Malformed("bad magic")));
    }

    #[test]
    fn header_rejects_truncated() {
        let bytes = encode_header(Some(b"x")).expect("encode");
        let err = decode_header(&bytes[..bytes.len() / 2]).expect_err("truncated rejected");
        assert!(matches!(err, ChunkError::Malformed(_)));
    }
}
