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
use minicbor::data::Type;
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

/// Wire shape of a section heading map `{0: stream_id}`.
#[derive(minicbor::Encode, minicbor::Decode)]
#[cbor(map)]
struct HeadingRepr<'a> {
    #[n(0)]
    #[cbor(with = "minicbor::bytes")]
    stream_id: &'a [u8],
}

/// Encode a per-stream section heading, recording the origin stream id once.
/// Emit one before the blocks of each stream.
///
/// # Errors
/// Returns [`ChunkError::Encode`] if minicbor serialization fails (never in
/// practice).
pub fn encode_section_heading(stream_id: &[u8]) -> Result<Bytes, ChunkError> {
    let repr = HeadingRepr { stream_id };
    Ok(Bytes::from(minicbor::to_vec(&repr)?))
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

/// Decode a whole chunk into per-stream sections for
/// [`EventImporter::import`](crate::import::EventImporter::import).
///
/// Walks the CBOR sequence: header first, then peek each item — a map starts a
/// new section, an array is a block for the current section. A torn tail
/// (end-of-input mid-item) stops cleanly and returns the valid prefix; any
/// other structural problem is [`ChunkError::Malformed`]. A per-block crc
/// mismatch is a non-fatal [`ImportBlock::Corrupt`], never an error.
///
/// # Errors
/// Returns [`ChunkError::Malformed`] on a bad/unknown header, a block before
/// any section heading, an unexpected top-level item, or a crc-valid body that
/// fails to decode.
pub fn decode_chunk(bytes: &[u8]) -> Result<Vec<StreamSection>, ChunkError> {
    let mut d = Decoder::new(bytes);
    let header: HeaderRepr = d
        .decode()
        .map_err(|_| ChunkError::Malformed("unreadable header"))?;
    validate_header(header.magic, header.format_version)?;

    let mut sections: Vec<StreamSection> = Vec::new();
    while d.position() < bytes.len() {
        match d.datatype() {
            Ok(Type::Map) => match d.decode::<HeadingRepr>() {
                Ok(heading) => sections.push(StreamSection {
                    origin: Bytes::copy_from_slice(heading.stream_id),
                    blocks: Vec::new(),
                }),
                Err(e) if e.is_end_of_input() => break,
                Err(_) => return Err(ChunkError::Malformed("malformed section heading")),
            },
            Ok(Type::Array) => match decode_block(&mut d) {
                Ok(Some(block)) => match sections.last_mut() {
                    Some(section) => section.blocks.push(block),
                    None => return Err(ChunkError::Malformed("block before section heading")),
                },
                Ok(None) => break,
                Err(hint) => return Err(ChunkError::Malformed(hint)),
            },
            Ok(_) => return Err(ChunkError::Malformed("unexpected item type")),
            Err(e) if e.is_end_of_input() => break,
            Err(_) => return Err(ChunkError::Malformed("decode error")),
        }
    }
    Ok(sections)
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
    use crate::envelope::pending_envelope;
    use crate::import::{Atomicity, EventImporter};
    use crate::store::RawEventStore;
    use crate::testing::InMemoryStore;
    use futures::StreamExt;

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

    /// Encode a full multi-stream chunk: header + per-stream heading + blocks.
    fn encode_chunk(origin: Option<&[u8]>, streams: &[(&[u8], Vec<PersistedEnvelope>)]) -> Bytes {
        let mut buf = Vec::new();
        buf.extend_from_slice(&encode_header(origin).expect("header"));
        for (stream_id, events) in streams {
            buf.extend_from_slice(&encode_section_heading(stream_id).expect("heading"));
            for e in events {
                buf.extend_from_slice(&encode_block(e).expect("block"));
            }
        }
        Bytes::from(buf)
    }

    #[test]
    fn decode_chunk_round_trips_multi_stream() {
        let a = vec![
            persisted(1, 1, "E", None, b"a1"),
            persisted(2, 1, "E", Some(b"m"), b"a2"),
        ];
        let b = vec![persisted(1, 2, "E", None, b"b1")];
        let chunk = encode_chunk(
            Some(b"dev-1"),
            &[(b"task-1".as_slice(), a), (b"task-2".as_slice(), b)],
        );

        let sections = decode_chunk(&chunk).expect("decode");
        assert_eq!(sections.len(), 2);

        assert_eq!(sections[0].origin.as_ref(), b"task-1");
        assert_eq!(sections[0].blocks.len(), 2);
        match (&sections[0].blocks[0], &sections[0].blocks[1]) {
            (ImportBlock::Event(e1), ImportBlock::Event(e2)) => {
                assert_eq!(e1.version().as_u64(), 1);
                assert_eq!(e1.payload(), b"a1");
                assert_eq!(e2.metadata(), Some(b"m".as_slice()));
                assert_eq!(e2.payload(), b"a2");
            }
            _ => panic!("expected two Event blocks"),
        }

        assert_eq!(sections[1].origin.as_ref(), b"task-2");
        match &sections[1].blocks[0] {
            ImportBlock::Event(e) => {
                assert_eq!(e.schema_version(), 2);
                assert_eq!(e.payload(), b"b1");
            }
            ImportBlock::Corrupt => panic!("expected Event"),
        }
    }

    #[test]
    fn decode_header_only_chunk_is_empty_vec() {
        let chunk = encode_header(None).expect("header");
        let sections = decode_chunk(&chunk).expect("decode");
        assert!(sections.is_empty());
    }

    #[test]
    fn decode_block_before_heading_is_malformed() {
        let mut chunk = encode_header(None).expect("header").to_vec();
        let event = persisted(1, 1, "E", None, b"x");
        chunk.extend_from_slice(&encode_block(&event).expect("block"));
        let err = decode_chunk(&chunk).expect_err("block before heading");
        assert!(matches!(
            err,
            ChunkError::Malformed("block before section heading")
        ));
    }

    #[test]
    fn decode_empty_section_then_stream() {
        let chunk = encode_chunk(
            None,
            &[
                (b"empty".as_slice(), vec![]),
                (b"real".as_slice(), vec![persisted(1, 1, "E", None, b"r")]),
            ],
        );
        let sections = decode_chunk(&chunk).expect("decode");
        assert_eq!(sections.len(), 2);
        assert!(sections[0].blocks.is_empty());
        assert_eq!(sections[0].origin.as_ref(), b"empty");
        assert_eq!(sections[1].blocks.len(), 1);
    }

    // ── Task 5: Defensive boundary — Corrupt vs Malformed ──────────────────

    #[test]
    fn flipped_body_byte_in_chunk_decodes_to_corrupt_block() {
        let chunk = encode_chunk(
            None,
            &[(b"s".as_slice(), vec![persisted(1, 1, "E", None, b"hello")])],
        );
        let mut v = chunk.to_vec();
        let last = v.len() - 1;
        v[last] ^= 0xFF;
        let sections = decode_chunk(&v).expect("framing intact");
        assert_eq!(sections.len(), 1);
        assert!(matches!(sections[0].blocks[0], ImportBlock::Corrupt));
    }

    #[test]
    fn bad_magic_chunk_is_malformed() {
        let mut v = encode_chunk(
            None,
            &[(b"s".as_slice(), vec![persisted(1, 1, "E", None, b"x")])],
        )
        .to_vec();
        let pos = v.iter().position(|&b| b == b'n').expect("magic");
        v[pos] = b'Z';
        assert!(matches!(
            decode_chunk(&v),
            Err(ChunkError::Malformed("bad magic"))
        ));
    }

    #[test]
    fn unknown_format_version_is_malformed() {
        let repr = HeaderRepr {
            magic: MAGIC,
            format_version: 2,
            origin: None,
        };
        let bytes = minicbor::to_vec(&repr).expect("encode");
        assert!(matches!(
            decode_chunk(&bytes),
            Err(ChunkError::Malformed("unknown format version"))
        ));
    }

    #[test]
    fn unexpected_top_level_item_is_malformed() {
        let mut v = encode_header(None).expect("header").to_vec();
        v.push(0x01); // CBOR uint 1 — neither map nor array
        assert!(matches!(
            decode_chunk(&v),
            Err(ChunkError::Malformed("unexpected item type"))
        ));
    }

    #[test]
    fn crc_valid_but_body_invalid_is_malformed() {
        // Craft a block whose body decodes to version 0 (illegal) with a
        // MATCHING crc → proves crc-pass-body-fail => Malformed (not Corrupt).
        let mut body = Vec::new();
        {
            let mut e = minicbor::Encoder::new(&mut body);
            e.map(4)
                .expect("map")
                .u32(0)
                .expect("k0")
                .u64(0)
                .expect("v0")
                .u32(1)
                .expect("k1")
                .u32(1)
                .expect("v1")
                .u32(2)
                .expect("k2")
                .str("E")
                .expect("v2")
                .u32(4)
                .expect("k4")
                .bytes(b"")
                .expect("v4");
        }
        let block = BlockRepr {
            crc: crc32c::crc32c(&body),
            body: &body,
        };
        let mut chunk = encode_header(None).expect("header").to_vec();
        chunk.extend_from_slice(&encode_section_heading(b"s").expect("heading"));
        chunk.extend_from_slice(&minicbor::to_vec(&block).expect("block"));
        assert!(matches!(
            decode_chunk(&chunk),
            Err(ChunkError::Malformed(_))
        ));
    }

    // ── Task 6: Lifecycle — incremental append / truncation / valid prefix ──

    /// The cumulative byte length after the header + heading + first N blocks.
    fn prefix_len_after_n_blocks(
        stream_id: &[u8],
        events: &[PersistedEnvelope],
        n: usize,
    ) -> usize {
        let mut len = encode_header(None).expect("header").len();
        len += encode_section_heading(stream_id).expect("heading").len();
        for e in events.iter().take(n) {
            len += encode_block(e).expect("block").len();
        }
        len
    }

    #[test]
    fn every_block_boundary_prefix_is_valid() {
        let events: Vec<_> = (1..=4).map(|v| persisted(v, 1, "E", None, b"p")).collect();
        let chunk = encode_chunk(None, &[(b"s".as_slice(), events.clone())]);
        for n in 0..=events.len() {
            let cut = prefix_len_after_n_blocks(b"s", &events, n);
            let sections = decode_chunk(&chunk[..cut]).expect("prefix valid");
            let got = sections.first().map_or(0, |s| s.blocks.len());
            assert_eq!(got, n, "prefix after {n} blocks must decode to {n} blocks");
        }
    }

    #[test]
    fn torn_final_block_is_dropped_earlier_survive() {
        let events: Vec<_> = (1..=3)
            .map(|v| persisted(v, 1, "E", None, b"payload"))
            .collect();
        let chunk = encode_chunk(None, &[(b"s".as_slice(), events)]);
        let sections = decode_chunk(&chunk[..chunk.len() - 1]).expect("valid prefix");
        assert_eq!(sections[0].blocks.len(), 2, "torn 3rd block dropped");
        assert!(matches!(sections[0].blocks[0], ImportBlock::Event(_)));
    }

    #[test]
    fn empty_input_is_malformed_header() {
        assert!(matches!(decode_chunk(&[]), Err(ChunkError::Malformed(_))));
    }

    // ── Task 7: Full pipeline — export → box → import ───────────────────────

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);

    impl core::fmt::Display for Tid {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }

    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    #[cfg(feature = "testing")]
    #[tokio::test]
    async fn export_box_import_round_trip_byte_equal_modulo_global_seq() {
        // Seed a source store with two streams.
        let src = InMemoryStore::new();
        for (sid, count) in [("task-1", 3u64), ("task-2", 2)] {
            for v in 1..=count {
                let pe = pending_envelope(Version::new(v).expect("nonzero"))
                    .event_type("E")
                    .payload(format!("{sid}-{v}").into_bytes())
                    .expect("valid payload")
                    .build();
                src.append(
                    &Tid(sid.into()),
                    Version::new(v - 1),
                    core::slice::from_ref(&pe),
                )
                .await
                .expect("append");
            }
        }

        // Export → box-encode into one chunk.
        let mut chunk = encode_header(Some(b"src")).expect("header").to_vec();
        for sid in ["task-1", "task-2"] {
            chunk.extend_from_slice(&encode_section_heading(sid.as_bytes()).expect("heading"));
            let mut s = src
                .read_stream(&Tid(sid.into()), Version::INITIAL)
                .await
                .expect("read");
            while let Some(item) = s.next().await {
                let e = item.expect("no read error");
                chunk.extend_from_slice(&encode_block(&e).expect("block"));
            }
        }

        // Box-decode → import into a fresh store under origin-namespaced ids.
        let sections = decode_chunk(&chunk).expect("decode");
        let dst = InMemoryStore::new();
        let route = |origin: &[u8]| Tid(format!("src:{}", String::from_utf8_lossy(origin)));
        let report = dst
            .import(&sections, route, Atomicity::PerStream)
            .await
            .expect("import");
        assert!(report.all_complete());

        // Verify byte-equality of payloads/versions modulo global_seq.
        for sid in ["task-1", "task-2"] {
            let target = Tid(format!("src:{sid}"));
            let got: Vec<(u64, Vec<u8>)> = dst
                .read_stream(&target, Version::INITIAL)
                .await
                .expect("read")
                .map(|r| {
                    let e = r.expect("no err");
                    (e.version().as_u64(), e.payload().to_vec())
                })
                .collect()
                .await;
            let expected: Vec<(u64, Vec<u8>)> = (1..=if sid == "task-1" { 3u64 } else { 2 })
                .map(|v| (v, format!("{sid}-{v}").into_bytes()))
                .collect();
            assert_eq!(got, expected, "stream {sid} round-trips");
        }
    }
}
