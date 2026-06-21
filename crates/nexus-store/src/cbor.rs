//! The default CBOR backup box — the bytes↔sections travel codec.
//!
//! Sits between Card 1's raw export (per-stream [`PersistedEnvelope`]s) and
//! Card 2's [`EventImporter`](crate::import::EventImporter). A chunk is a CBOR
//! sequence (RFC 8742): a header map, then per-stream section headings each
//! followed by block arrays `[crc32c, body]`. See
//! `docs/plans/2026-06-21-export-import-cbor-box-design.md`.

use core::convert::Infallible;
use core::ops::Range;

use bytes::Bytes;
use minicbor::Decoder;
use thiserror::Error;

use crate::envelope::PersistedEnvelope;
use crate::import::{ImportBlock, StreamSection};
use crate::store::GlobalSeq;
use crate::value::SchemaVersion;
use nexus::Version;

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

/// Wire shape of a block body map. `global_seq` is deliberately absent —
/// store-local, restamped on import.
#[derive(minicbor::Encode, minicbor::Decode)]
#[cbor(map)]
struct BodyRepr<'a> {
    #[n(0)]
    version: u64,
    #[n(1)]
    schema_version: u32,
    #[n(2)]
    event_type: &'a str,
    #[n(3)]
    #[cbor(with = "minicbor::bytes")]
    metadata: Option<&'a [u8]>,
    #[n(4)]
    #[cbor(with = "minicbor::bytes")]
    payload: &'a [u8],
}

/// Wire shape of a block array `[crc32c, body]`. Encode-only (decode is manual
/// so the crc is checked before the body is trusted).
#[derive(minicbor::Encode)]
#[cbor(array)]
struct BlockRepr<'a> {
    #[n(0)]
    crc: u32,
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    body: &'a [u8],
}

/// Encode one event as a block `[crc32c(body), body]`. The crc covers exactly
/// the body bstr bytes.
///
/// # Errors
/// Returns [`ChunkError::Encode`] if minicbor serialization fails (never in
/// practice).
pub fn encode_block(event: &PersistedEnvelope) -> Result<Bytes, ChunkError> {
    let body = BodyRepr {
        version: event.version().as_u64(),
        schema_version: event.schema_version(),
        event_type: event.event_type(),
        metadata: event.metadata(),
        payload: event.payload(),
    };
    let body_bytes = minicbor::to_vec(&body)?;
    let block = BlockRepr {
        crc: crc32c::crc32c(&body_bytes),
        body: &body_bytes,
    };
    Ok(Bytes::from(minicbor::to_vec(&block)?))
}

/// Decode one block from the decoder's current position.
///
/// `Ok(Some(block))` = decoded (Event or Corrupt). `Ok(None)` = torn tail
/// (end-of-input mid-item → caller stops, valid prefix). `Err(hint)` = a
/// structural violation the caller turns into [`ChunkError::Malformed`].
#[allow(
    dead_code,
    reason = "wired into decode_chunk in Task 4; called from tests now"
)]
fn decode_block(d: &mut Decoder<'_>) -> Result<Option<ImportBlock>, &'static str> {
    match d.array() {
        Ok(Some(2)) => {}
        Ok(Some(_)) => return Err("block array must have exactly 2 elements"),
        Ok(None) => return Err("indefinite-length block array"),
        Err(e) if e.is_end_of_input() => return Ok(None),
        Err(_) => return Err("malformed block array"),
    }
    let crc = match d.u32() {
        Ok(c) => c,
        Err(e) if e.is_end_of_input() => return Ok(None),
        Err(_) => return Err("malformed block crc"),
    };
    let body: &[u8] = match d.bytes() {
        Ok(b) => b,
        Err(e) if e.is_end_of_input() => return Ok(None),
        Err(_) => return Err("malformed block body bytes"),
    };
    if crc32c::crc32c(body) != crc {
        // crc failed → bytes untrusted → Corrupt, do NOT decode the body.
        return Ok(Some(ImportBlock::Corrupt));
    }
    // crc passed → body bytes are intact-as-written; any decode failure is a
    // format violation, not bit-rot → Malformed (caller's job).
    let parsed: BodyRepr = minicbor::decode(body).map_err(|_| "crc-valid body failed to decode")?;
    let envelope = reconstruct(&parsed).ok_or("crc-valid body has invalid fields")?;
    Ok(Some(ImportBlock::Event(envelope)))
}

/// Rebuild a [`PersistedEnvelope`] from a decoded body. Returns `None` on any
/// field-level violation (version 0, schema 0, oversize, range overflow); the
/// caller maps that to `Malformed`. `global_seq` is a placeholder — import
/// ignores it.
#[allow(
    dead_code,
    reason = "called by decode_block, wired into decode_chunk in Task 4"
)]
fn reconstruct(body: &BodyRepr<'_>) -> Option<PersistedEnvelope> {
    let version = Version::new(body.version)?;
    let schema = SchemaVersion::from_u32(body.schema_version).ok()?;
    let event_type = body.event_type.as_bytes();
    let et_end = u32::try_from(event_type.len()).ok()?;
    let mut buf = Vec::with_capacity(
        event_type.len() + body.metadata.map_or(0, <[u8]>::len) + body.payload.len(),
    );
    buf.extend_from_slice(event_type);
    let et_range: Range<u32> = 0..et_end;
    let meta_range = match body.metadata {
        Some(m) => {
            let start = u32::try_from(buf.len()).ok()?;
            buf.extend_from_slice(m);
            let end = u32::try_from(buf.len()).ok()?;
            Some(start..end)
        }
        None => None,
    };
    let pl_start = u32::try_from(buf.len()).ok()?;
    buf.extend_from_slice(body.payload);
    let pl_end = u32::try_from(buf.len()).ok()?;
    PersistedEnvelope::try_new(
        version,
        GlobalSeq::INITIAL,
        Bytes::from(buf),
        schema,
        et_range,
        pl_start..pl_end,
        meta_range,
    )
    .ok()
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

    /// Build a `PersistedEnvelope` directly: backing buffer = [`event_type` | metadata? | payload].
    fn persisted(
        version: u64,
        schema: u32,
        event_type: &str,
        metadata: Option<&[u8]>,
        payload: &[u8],
    ) -> PersistedEnvelope {
        let mut buf = Vec::new();
        buf.extend_from_slice(event_type.as_bytes());
        let et_end = u32::try_from(buf.len()).expect("fits");
        let meta_range = metadata.map(|m| {
            let start = u32::try_from(buf.len()).expect("fits");
            buf.extend_from_slice(m);
            start..u32::try_from(buf.len()).expect("fits")
        });
        let pl_start = u32::try_from(buf.len()).expect("fits");
        buf.extend_from_slice(payload);
        let pl_end = u32::try_from(buf.len()).expect("fits");
        PersistedEnvelope::try_new(
            Version::new(version).expect("nonzero"),
            GlobalSeq::new(version).expect("nonzero"),
            Bytes::from(buf),
            SchemaVersion::from_u32(schema).expect("nonzero"),
            0..et_end,
            pl_start..pl_end,
            meta_range,
        )
        .expect("valid persisted")
    }

    /// Decode a single block from a freshly-encoded block buffer.
    fn decode_one_block(bytes: &[u8]) -> ImportBlock {
        let mut d = Decoder::new(bytes);
        decode_block(&mut d)
            .expect("not malformed")
            .expect("not torn")
    }

    #[test]
    fn block_round_trips_all_fields() {
        let event = persisted(7, 3, "AccountOpened", Some(b"hlc=42"), b"balance:100");
        let bytes = encode_block(&event).expect("encode");
        match decode_one_block(&bytes) {
            ImportBlock::Event(got) => {
                assert_eq!(got.version().as_u64(), 7);
                assert_eq!(got.schema_version(), 3);
                assert_eq!(got.event_type(), "AccountOpened");
                assert_eq!(got.metadata(), Some(b"hlc=42".as_slice()));
                assert_eq!(got.payload(), b"balance:100");
            }
            ImportBlock::Corrupt => panic!("expected Event, got Corrupt"),
        }
    }

    #[test]
    fn block_round_trips_without_metadata_and_empty_payload() {
        let event = persisted(1, 1, "E", None, b"");
        let bytes = encode_block(&event).expect("encode");
        match decode_one_block(&bytes) {
            ImportBlock::Event(got) => {
                assert_eq!(got.metadata(), None);
                assert_eq!(got.payload(), b"");
                assert_eq!(got.version().as_u64(), 1);
            }
            ImportBlock::Corrupt => panic!("expected Event"),
        }
    }

    #[test]
    fn block_with_flipped_body_byte_is_corrupt() {
        let event = persisted(2, 1, "E", None, b"hello");
        let bytes = encode_block(&event).expect("encode");
        let mut v = bytes.to_vec();
        let last = v.len() - 1;
        v[last] ^= 0xFF;
        assert!(matches!(decode_one_block(&v), ImportBlock::Corrupt));
    }

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
