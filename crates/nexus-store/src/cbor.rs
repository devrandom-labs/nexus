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

/// Stub — replaced in Task 2.
///
/// # Errors
///
/// Returns [`ChunkError::Encode`] if the CBOR serialization fails (infallible
/// in practice for the stub path).
pub const fn encode_header(_origin: Option<&[u8]>) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}

/// Stub — replaced in Task 2.
///
/// # Errors
///
/// Returns [`ChunkError::Malformed`] if the chunk header bytes cannot be
/// decoded.
pub const fn decode_header(_bytes: &[u8]) -> Result<ChunkHeader, ChunkError> {
    Err(ChunkError::Malformed("unimplemented"))
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
