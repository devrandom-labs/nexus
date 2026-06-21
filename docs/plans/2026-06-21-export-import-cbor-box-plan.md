# Export/Import Card 3 — CBOR backup box Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the default CBOR backup box — the bytes↔sections codec between Card 1's raw export and Card 2's `EventImporter` — in `crates/nexus-store/src/cbor.rs` behind a new `cbor` feature.

**Architecture:** A chunk is a CBOR sequence (RFC 8742): a header map, then per-stream section headings (maps) each followed by block arrays `[crc32c, body]`. Encode = three stateless free functions returning appendable `Bytes`. Decode = a whole-buffer walk that peeks each item's CBOR major type (map ⇒ heading, array ⇒ block) and rebuilds `PersistedEnvelope`s, classifying failures as `Corrupt` (crc mismatch, localized) or `Malformed` (structural, whole-chunk). See `docs/plans/2026-06-21-export-import-cbor-box-design.md`.

**Tech Stack:** Rust 2024, `minicbor` 2.2.2 (derive, no_std/no-serde) for CBOR, `crc32c` for the Castagnoli per-block checksum, `bytes::Bytes`, `thiserror`, `insta` for golden bytes, `proptest` for VOPR.

---

## File structure

- **Create** `crates/nexus-store/src/cbor.rs` — the entire box: constants, `ChunkError`, `ChunkHeader`, the `minicbor::Encode`/`Decode` repr structs (`HeaderRepr`, `HeadingRepr`, `BodyRepr`, `BlockRepr`), the public `encode_header` / `encode_section_heading` / `encode_block` / `decode_header` / `decode_chunk`, the private `validate_header` / `decode_block` / `reconstruct`, and `#[cfg(test)] mod tests`.
- **Modify** `crates/nexus-store/Cargo.toml` — add the `cbor` feature, `minicbor` + `crc32c` optional deps, and `cbor` to the self dev-dependency feature list. Add `insta` dev-dep if absent.
- **Modify** `crates/nexus-store/src/lib.rs` — `#[cfg(feature = "cbor")] pub mod cbor;` + re-exports.
- **Modify** root `Cargo.toml` — `[workspace.dependencies]` entries for `minicbor`, `crc32c` (and `insta` if absent).

Everything box-related is one file. It is small, cohesive, and gated — no reason to split.

---

## Task 1: Dependencies, feature wiring, and module skeleton

**Files:**
- Modify: root `Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/nexus-store/Cargo.toml`
- Create: `crates/nexus-store/src/cbor.rs`
- Modify: `crates/nexus-store/src/lib.rs`

- [ ] **Step 1: Add the dependencies via `cargo add` (never hand-write versions).**

```bash
cd /Users/joel/Code/devrandom/nexus
nix develop -c cargo add minicbor --features derive,alloc -p nexus-store --optional
nix develop -c cargo add crc32c -p nexus-store --optional
# insta is dev-only; check first, add only if missing:
nix develop -c cargo add insta --dev -p nexus-store
nix develop -c cargo hakari generate
```

Expected: `minicbor`, `crc32c` land in `[workspace.dependencies]` and as `optional = true` under `nexus-store` `[dependencies]`; `insta` under `[dev-dependencies]`. If `cargo add` places versions only in the crate manifest, move the version to `[workspace.dependencies]` and set `workspace = true` in the crate manifest to match the repo convention (then re-run `cargo hakari generate`).

- [ ] **Step 2: Add the `cbor` feature and extend the self dev-dependency.**

In `crates/nexus-store/Cargo.toml` `[features]`, add:

```toml
# Default CBOR backup box (issue #145, Card 3). Implies export+import; the
# inverse never holds (Agency enables export/import and brings its own CESR box,
# pulling no CBOR).
cbor = ["import", "export", "dep:minicbor", "dep:crc32c"]
```

In `[dev-dependencies]`, extend the self dep so box tests run under `nix flake check` (the gate runs default features only):

```toml
nexus-store = { path = ".", features = ["testing", "export", "import", "cbor"] }
```

- [ ] **Step 3: Create the module skeleton `crates/nexus-store/src/cbor.rs`.**

```rust
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
use minicbor::data::Type;
use minicbor::Decoder;
use nexus::Version;
use thiserror::Error;

use crate::envelope::PersistedEnvelope;
use crate::import::{ImportBlock, StreamSection};
use crate::store::GlobalSeq;
use crate::value::SchemaVersion;

/// Chunk magic — `"nxch"` (h'6e786368').
const MAGIC: &[u8] = b"nxch";
/// The only format version this build emits and accepts.
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
    /// The chunk format version (always [`FORMAT_VERSION`] for a chunk this
    /// build accepts).
    pub format_version: u32,
    /// The chunk-level producer/device id, if the encoder recorded one.
    pub origin: Option<Bytes>,
}
```

- [ ] **Step 4: Wire `lib.rs`.**

After the `import` module gate (line ~92) add:

```rust
#[cfg(feature = "cbor")]
pub mod cbor;
```

In the re-export block, after the `import` re-exports (line ~132) add:

```rust
#[cfg(feature = "cbor")]
pub use cbor::{
    decode_chunk, decode_header, encode_block, encode_header, encode_section_heading, ChunkError,
    ChunkHeader,
};
```

(The free functions don't exist yet — Step 5 adds temporary stubs so this compiles, or comment the function names out of the re-export until Task 2. Prefer stubs.)

- [ ] **Step 5: Add temporary stubs so the crate compiles.**

Append to `cbor.rs` (these are replaced in later tasks):

```rust
/// Stub — replaced in Task 2.
pub fn encode_header(_origin: Option<&[u8]>) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}
/// Stub — replaced in Task 2.
pub fn decode_header(_bytes: &[u8]) -> Result<ChunkHeader, ChunkError> {
    Err(ChunkError::Malformed("unimplemented"))
}
/// Stub — replaced in Task 3.
pub fn encode_section_heading(_stream_id: &[u8]) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}
/// Stub — replaced in Task 3.
pub fn encode_block(_event: &PersistedEnvelope) -> Result<Bytes, ChunkError> {
    Ok(Bytes::new())
}
/// Stub — replaced in Task 4.
pub fn decode_chunk(_bytes: &[u8]) -> Result<Vec<StreamSection>, ChunkError> {
    Ok(Vec::new())
}
```

- [ ] **Step 6: Verify it compiles (and unused imports are tolerated for now).**

Run: `nix develop -c cargo build -p nexus-store --features cbor,testing`
Expected: builds. Warnings about unused imports/items are acceptable at this checkpoint (they're consumed in Tasks 2–4). If clippy `--deny warnings` complains, add a temporary `#[allow(unused_imports, dead_code)]` on the module and remove it in Task 4's final step.

- [ ] **Step 7: Commit.**

```bash
cd /Users/joel/Code/devrandom/nexus
nix develop -c cargo fmt --all
git add crates/nexus-store/Cargo.toml crates/nexus-store/src/cbor.rs crates/nexus-store/src/lib.rs Cargo.toml crates/workspace-hack/
git commit -m "$(cat <<'EOF'
feat(store): scaffold cbor backup box feature + module skeleton

Card 3 of export/import (#145): the default CBOR travel codec. Adds the
`cbor` feature (implies export+import; never the reverse), pins minicbor +
crc32c, and lands ChunkError/ChunkHeader plus stubbed public functions.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the pre-commit hook (`nix flake check`) to pass; confirm HEAD changed (`git log -1 --oneline`).

---

## Task 2: Header — `encode_header` + `decode_header` round-trip

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the failing tests.** Add to (or create) `#[cfg(test)] mod tests` at the bottom of `cbor.rs`:

```rust
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
        // A valid CBOR map with a wrong magic value.
        let mut wrong = encode_header(None).expect("encode");
        // Flip a magic byte: locate 'n' (0x6e) of "nxch" and corrupt it.
        let mut v = wrong.to_vec();
        let pos = v.iter().position(|&b| b == b'n').expect("magic present");
        v[pos] = b'X';
        wrong = Bytes::from(v);
        let err = decode_header(&wrong).expect_err("bad magic rejected");
        assert!(matches!(err, ChunkError::Malformed("bad magic")));
    }

    #[test]
    fn header_rejects_truncated() {
        let bytes = encode_header(Some(b"x")).expect("encode");
        let err = decode_header(&bytes[..bytes.len() / 2]).expect_err("truncated rejected");
        assert!(matches!(err, ChunkError::Malformed(_)));
    }
}
```

- [ ] **Step 2: Run, verify failure.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::header_ -- --nocapture`
Expected: FAIL (stubs return empty / unimplemented).

- [ ] **Step 3: Implement the header repr + functions.** Add near the top of `cbor.rs` (after `ChunkHeader`):

```rust
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
```

Replace the `encode_header` / `decode_header` stubs:

```rust
/// Encode the chunk header. Call once, at the start of a chunk. `origin` is an
/// optional chunk-level producer/device id (omitted from the map when `None`).
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
pub fn decode_header(bytes: &[u8]) -> Result<ChunkHeader, ChunkError> {
    let repr: HeaderRepr =
        minicbor::decode(bytes).map_err(|_| ChunkError::Malformed("unreadable header"))?;
    validate_header(repr.magic, repr.format_version)?;
    Ok(ChunkHeader {
        format_version: repr.format_version,
        origin: repr.origin.map(Bytes::copy_from_slice),
    })
}
```

**Verification note (minicbor `with` on `Option`):** if `#[cbor(with = "minicbor::bytes")]` on `origin: Option<&[u8]>` fails to compile, change the field to `Option<&'a minicbor::bytes::ByteSlice>` (drop the `with`), set `origin: origin.map(<&minicbor::bytes::ByteSlice>::from)` in `encode_header`, and read it back with `repr.origin.map(|b| Bytes::copy_from_slice(b.as_ref()))`.

- [ ] **Step 4: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::header_`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
feat(store): cbor box header encode/decode + validation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 3: Block body — `encode_block` + private `decode_block`/`reconstruct` round-trip

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the failing tests.** Add to `mod tests`:

```rust
    use crate::envelope::{pending_envelope, PersistedEnvelope};
    use crate::store::GlobalSeq;
    use crate::value::SchemaVersion;

    /// Build a PersistedEnvelope directly (mirrors import.rs's test helper):
    /// backing buffer = [event_type | metadata? | payload].
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
        // Flip the last byte (inside the body bstr → crc mismatch).
        let mut v = bytes.to_vec();
        let last = v.len() - 1;
        v[last] ^= 0xFF;
        assert!(matches!(decode_one_block(&v), ImportBlock::Corrupt));
    }
```

- [ ] **Step 2: Run, verify failure.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::block_`
Expected: FAIL (`decode_block` / real `encode_block` not defined).

- [ ] **Step 3: Implement the body/block reprs, `encode_block`, `decode_block`, `reconstruct`.** Add to `cbor.rs`:

```rust
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
        // crc failed → header untrusted → Corrupt, do NOT decode the body.
        return Ok(Some(ImportBlock::Corrupt));
    }
    // crc passed → body bytes are intact-as-written; any decode failure is a
    // format violation, not bit-rot → Malformed (caller's job).
    let parsed: BodyRepr =
        minicbor::decode(body).map_err(|_| "crc-valid body failed to decode")?;
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
    let mut buf =
        Vec::with_capacity(event_type.len() + body.metadata.map_or(0, <[u8]>::len) + body.payload.len());
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
```

- [ ] **Step 4: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::block_`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
feat(store): cbor box block encode + crc-checked block decode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 4: `encode_section_heading` + `decode_chunk` (the integrator)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the failing test (Category 1 — sequence/protocol).** Add to `mod tests`:

```rust
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
        // header immediately followed by a block (no heading).
        let mut chunk = encode_header(None).expect("header").to_vec();
        let event = persisted(1, 1, "E", None, b"x");
        chunk.extend_from_slice(&encode_block(&event).expect("block"));
        let err = decode_chunk(&chunk).expect_err("block before heading");
        assert!(matches!(err, ChunkError::Malformed("block before section heading")));
    }

    #[test]
    fn decode_empty_section_then_stream() {
        // A heading with no blocks, then a real stream.
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
```

- [ ] **Step 2: Run, verify failure.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::decode_`
Expected: FAIL (stub `decode_chunk` returns empty; `encode_section_heading` is a stub).

- [ ] **Step 3: Implement `HeadingRepr`, `encode_section_heading`, `decode_chunk`.** Replace the stubs:

```rust
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
pub fn encode_section_heading(stream_id: &[u8]) -> Result<Bytes, ChunkError> {
    let repr = HeadingRepr { stream_id };
    Ok(Bytes::from(minicbor::to_vec(&repr)?))
}

/// Decode a whole chunk into per-stream sections for
/// [`EventImporter::import`](crate::import::EventImporter::import).
///
/// Walks the CBOR sequence: header first, then peek each item — a map starts a
/// new section, an array is a block for the current section. A torn tail
/// (end-of-input mid-item) stops cleanly and returns the valid prefix; any
/// other structural problem is [`ChunkError::Malformed`]. A per-block crc
/// mismatch is a non-fatal [`ImportBlock::Corrupt`], never an error.
pub fn decode_chunk(bytes: &[u8]) -> Result<Vec<StreamSection>, ChunkError> {
    let mut d = Decoder::new(bytes);
    let header: HeaderRepr =
        d.decode().map_err(|_| ChunkError::Malformed("unreadable header"))?;
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
                Ok(None) => break, // torn block tail → valid prefix
                Err(hint) => return Err(ChunkError::Malformed(hint)),
            },
            Ok(_) => return Err(ChunkError::Malformed("unexpected item type")),
            Err(e) if e.is_end_of_input() => break,
            Err(_) => return Err(ChunkError::Malformed("decode error")),
        }
    }
    Ok(sections)
}
```

Remove any temporary `#[allow(unused_imports, dead_code)]` added in Task 1.

- [ ] **Step 4: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::decode_`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
feat(store): cbor box section heading + decode_chunk sequence walk

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 5: Defensive boundary — Corrupt vs Malformed classification (Category 3)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the failing tests.** Add to `mod tests`:

```rust
    #[test]
    fn flipped_body_byte_in_chunk_decodes_to_corrupt_block() {
        let chunk = encode_chunk(None, &[(b"s".as_slice(), vec![persisted(1, 1, "E", None, b"hello")])]);
        let mut v = chunk.to_vec();
        // Corrupt the very last byte (inside the only block's body).
        let last = v.len() - 1;
        v[last] ^= 0xFF;
        let sections = decode_chunk(&v).expect("framing intact");
        assert_eq!(sections.len(), 1);
        assert!(matches!(sections[0].blocks[0], ImportBlock::Corrupt));
    }

    #[test]
    fn bad_magic_chunk_is_malformed() {
        let mut v = encode_chunk(None, &[(b"s".as_slice(), vec![persisted(1, 1, "E", None, b"x")])]).to_vec();
        let pos = v.iter().position(|&b| b == b'n').expect("magic");
        v[pos] = b'Z';
        assert!(matches!(decode_chunk(&v), Err(ChunkError::Malformed("bad magic"))));
    }

    #[test]
    fn unknown_format_version_is_malformed() {
        // Hand-build a header map with format_version = 2.
        let repr = HeaderRepr { magic: MAGIC, format_version: 2, origin: None };
        let bytes = minicbor::to_vec(&repr).expect("encode");
        assert!(matches!(
            decode_chunk(&bytes),
            Err(ChunkError::Malformed("unknown format version"))
        ));
    }

    #[test]
    fn unexpected_top_level_item_is_malformed() {
        // header followed by a bare uint (neither map nor array).
        let mut v = encode_header(None).expect("header").to_vec();
        v.push(0x01); // CBOR uint 1
        assert!(matches!(
            decode_chunk(&v),
            Err(ChunkError::Malformed("unexpected item type"))
        ));
    }

    #[test]
    fn crc_valid_but_body_invalid_is_malformed() {
        // Craft a block whose body decodes to version 0 (illegal), with a
        // MATCHING crc, proving crc-pass-body-fail => Malformed (not Corrupt).
        let mut body = Vec::new();
        {
            // body map {0: 0u64, 1: 1u32, 2: "E", 4: ""} — version 0 is illegal.
            let mut e = minicbor::Encoder::new(&mut body);
            e.map(4).expect("map")
                .u32(0).expect("k0").u64(0).expect("v0")
                .u32(1).expect("k1").u32(1).expect("v1")
                .u32(2).expect("k2").str("E").expect("v2")
                .u32(4).expect("k4").bytes(b"").expect("v4");
        }
        let block = BlockRepr { crc: crc32c::crc32c(&body), body: &body };
        let mut chunk = encode_header(None).expect("header").to_vec();
        chunk.extend_from_slice(&encode_section_heading(b"s").expect("heading"));
        chunk.extend_from_slice(&minicbor::to_vec(&block).expect("block"));
        assert!(matches!(decode_chunk(&chunk), Err(ChunkError::Malformed(_))));
    }
```

- [ ] **Step 2: Run.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests`
Expected: these new tests PASS already if Tasks 2–4 are correct (this task is verification-by-test of the trust model). If `crc_valid_but_body_invalid_is_malformed` fails, the `reconstruct` version check is wrong — fix it.

- [ ] **Step 3: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
test(store): cbor box defensive boundary — corrupt vs malformed

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 6: Lifecycle — incremental append / truncation / valid prefix (Category 2)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the tests.** Add to `mod tests`:

```rust
    /// The cumulative byte length after the header + heading + first N blocks.
    fn prefix_len_after_n_blocks(stream_id: &[u8], events: &[PersistedEnvelope], n: usize) -> usize {
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
        let events: Vec<_> = (1..=3).map(|v| persisted(v, 1, "E", None, b"payload")).collect();
        let chunk = encode_chunk(None, &[(b"s".as_slice(), events.clone())]);
        // Cut one byte short of the end → the last block is torn.
        let sections = decode_chunk(&chunk[..chunk.len() - 1]).expect("valid prefix");
        assert_eq!(sections[0].blocks.len(), 2, "torn 3rd block dropped");
        assert!(matches!(sections[0].blocks[0], ImportBlock::Event(_)));
    }

    #[test]
    fn empty_input_is_malformed_header() {
        assert!(matches!(decode_chunk(&[]), Err(ChunkError::Malformed(_))));
    }
```

- [ ] **Step 2: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests`
Expected: PASS. If `every_block_boundary_prefix_is_valid` fails at `n=0` (header only) the clean-end check is wrong; if it fails mid-way the torn detection in `decode_block` is wrong.

- [ ] **Step 3: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
test(store): cbor box lifecycle — valid-at-every-prefix + torn tail

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 7: Full pipeline — export → box → import (Category 1, end-to-end)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the test.** Add to `mod tests`:

```rust
    use crate::import::{Atomicity, EventImporter};
    use crate::store::RawEventStore;
    use crate::testing::InMemoryStore;
    use futures::StreamExt;

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

    #[tokio::test]
    async fn export_box_import_round_trip_byte_equal_modulo_global_seq() {
        use crate::envelope::pending_envelope;
        // Seed a source store with two streams.
        let src = InMemoryStore::new();
        for (sid, count) in [("task-1", 3u64), ("task-2", 2)] {
            for v in 1..=count {
                let pe = pending_envelope(Version::new(v).expect("nz"))
                    .event_type("E")
                    .payload(format!("{sid}-{v}").into_bytes())
                    .expect("payload")
                    .build();
                src.append(&Tid(sid.into()), Version::new(v - 1), &[pe])
                    .await
                    .expect("append");
            }
        }

        // Export → box-encode into one chunk.
        let mut chunk = encode_header(Some(b"src")).expect("header").to_vec();
        for sid in ["task-1", "task-2"] {
            chunk.extend_from_slice(&encode_section_heading(sid.as_bytes()).expect("heading"));
            let mut s = src.read_stream(&Tid(sid.into()), Version::INITIAL).await.expect("read");
            while let Some(item) = s.next().await {
                let e = item.expect("no read error");
                chunk.extend_from_slice(&encode_block(&e).expect("block"));
            }
        }

        // Box-decode → import into a fresh store under origin-namespaced ids.
        let sections = decode_chunk(&chunk).expect("decode");
        let dst = InMemoryStore::new();
        let route = |origin: &[u8]| Tid(format!("src:{}", String::from_utf8_lossy(origin)));
        let report = dst.import(&sections, route, Atomicity::PerStream).await.expect("import");
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
            let expected: Vec<(u64, Vec<u8>)> = (1..=if sid == "task-1" { 3 } else { 2 })
                .map(|v| (v, format!("{sid}-{v}").into_bytes()))
                .collect();
            assert_eq!(got, expected, "stream {sid} round-trips");
        }
    }
```

- [ ] **Step 2: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::export_box_import_round_trip`
Expected: PASS.

- [ ] **Step 3: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs
git commit -m "$(cat <<'EOF'
test(store): cbor box full pipeline export -> box -> import round-trip

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 8: insta golden bytes (format drift guard)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the snapshot tests.** Add to `mod tests`:

```rust
    #[test]
    fn golden_header_bytes() {
        let bytes = encode_header(Some(b"dev")).expect("header");
        insta::assert_snapshot!("header_with_origin_hex", hex_of(&bytes));
    }

    #[test]
    fn golden_block_bytes() {
        let event = persisted(1, 1, "E", None, b"hi");
        let bytes = encode_block(&event).expect("block");
        insta::assert_snapshot!("block_v1_hex", hex_of(&bytes));
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
    }
```

- [ ] **Step 2: Generate and review the snapshots.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::golden_`
Expected: FAIL (snapshots not yet accepted). Inspect the pending `.snap.new` files under `crates/nexus-store/src/snapshots/` and **manually verify the hex against the spec**: header must start `a3` (3-entry map) then `00 44 6e786368` (key 0, bstr len 4, "nxch"), `01 01` (key 1 = 1), `02 43 646576` (key 2, bstr len 3, "dev"). Block must be `82` (array 2) then `1a XXXXXXXX` (uint crc) then `58 LL ..body..`. Then accept:

```bash
nix develop -c cargo insta accept
```

If `cargo insta` is not on PATH, accept by renaming each `*.snap.new` → `*.snap` after manual verification.

- [ ] **Step 3: Re-run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::golden_`
Expected: PASS.

- [ ] **Step 4: Commit (include the snapshot files).**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs crates/nexus-store/src/snapshots/
git commit -m "$(cat <<'EOF'
test(store): cbor box insta golden bytes for header + block

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 9: VOPR round-trip + crc-mutation property tests (proptest)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the property tests.** Add to `mod tests`:

```rust
    use proptest::prelude::*;

    /// (version, schema, event_type, metadata?, payload) — boundary-inclusive.
    fn event_strategy() -> impl Strategy<Value = (u64, u32, String, Option<Vec<u8>>, Vec<u8>)> {
        (
            prop_oneof![Just(1u64), Just(2), Just(u64::MAX - 1), Just(u64::MAX), 1u64..1000],
            prop_oneof![Just(1u32), Just(7), Just(u32::MAX), 1u32..100],
            prop_oneof![Just(String::new()), Just("E".to_owned()), "[A-Za-z]{0,40}"],
            prop_oneof![Just(None), proptest::collection::vec(any::<u8>(), 1..32).prop_map(Some)],
            proptest::collection::vec(any::<u8>(), 0..64),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn vopr_chunk_round_trips(
            origin in prop_oneof![Just(None), proptest::collection::vec(any::<u8>(), 1..16).prop_map(Some)],
            streams in proptest::collection::vec(
                (proptest::collection::vec(any::<u8>(), 0..12),
                 proptest::collection::vec(event_strategy(), 0..5)),
                0..4,
            ),
        ) {
            // Build sequential versions per stream (import requires contiguity);
            // but VOPR here checks the BOX (encode/decode), not import — so use
            // each tuple's version verbatim and assert decode mirrors encode.
            let built: Vec<(Vec<u8>, Vec<PersistedEnvelope>)> = streams
                .iter()
                .map(|(sid, evs)| {
                    let events = evs.iter().map(|(v, sc, et, md, pl)| {
                        persisted(*v, *sc, et, md.as_deref(), pl)
                    }).collect();
                    (sid.clone(), events)
                })
                .collect();

            let refs: Vec<(&[u8], Vec<PersistedEnvelope>)> =
                built.iter().map(|(s, e)| (s.as_slice(), e.clone())).collect();
            let chunk = encode_chunk(origin.as_deref(), &refs);
            let sections = decode_chunk(&chunk).expect("decode");

            prop_assert_eq!(sections.len(), built.len());
            for (section, (sid, events)) in sections.iter().zip(built.iter()) {
                prop_assert_eq!(section.origin.as_ref(), sid.as_slice());
                prop_assert_eq!(section.blocks.len(), events.len());
                for (block, original) in section.blocks.iter().zip(events.iter()) {
                    match block {
                        ImportBlock::Event(got) => {
                            prop_assert_eq!(got.version(), original.version());
                            prop_assert_eq!(got.schema_version(), original.schema_version());
                            prop_assert_eq!(got.event_type(), original.event_type());
                            prop_assert_eq!(got.metadata(), original.metadata());
                            prop_assert_eq!(got.payload(), original.payload());
                        }
                        ImportBlock::Corrupt => prop_assert!(false, "unexpected corrupt"),
                    }
                }
            }
        }

        #[test]
        fn vopr_single_byte_body_mutation_is_corrupt_never_silently_wrong(
            (v, sc, et, md, pl) in event_strategy(),
            flip_pick in any::<prop::sample::Index>(),
        ) {
            let event = persisted(v, sc, &et, md.as_deref(), &pl);
            let block = encode_block(&event).expect("block");
            // Locate the body bstr span and flip one byte inside it.
            let mut v = block.to_vec();
            // The body is the trailing bstr; flipping any byte from the array's
            // 2nd element onward lands in crc or body — both detectable. Pick a
            // byte in the back half (overwhelmingly body) to assert Corrupt.
            let start = v.len() / 2;
            let idx = start + flip_pick.index(v.len() - start);
            v[idx] ^= 0xFF;
            let mut d = Decoder::new(&v);
            match decode_block(&mut d) {
                Ok(Some(ImportBlock::Event(got))) => {
                    // If it still decodes as Event, it MUST be byte-identical to
                    // the original (the flip hit padding/length that re-decodes
                    // the same) — never silently different content.
                    prop_assert_eq!(got.payload(), event.payload());
                    prop_assert_eq!(got.event_type(), event.event_type());
                }
                Ok(Some(ImportBlock::Corrupt)) | Ok(None) | Err(_) => {} // all acceptable
            }
        }
    }
```

- [ ] **Step 2: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests::vopr_`
Expected: PASS. If `vopr_chunk_round_trips` finds a shrinking counterexample, the round-trip is broken for that boundary (e.g. empty event_type, `u64::MAX` version) — fix `encode_block`/`reconstruct`, do not weaken the strategy.

- [ ] **Step 3: Commit (include any proptest regression seed).**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/cbor.rs crates/nexus-store/src/cbor.proptest-regressions 2>/dev/null
git add crates/nexus-store/
git commit -m "$(cat <<'EOF'
test(store): cbor box VOPR round-trip + crc single-byte-mutation property

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 10: Defensive fuzz (never-panic) + forward-compat unknown key

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Write the tests.** Add to `mod tests`:

```rust
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn decode_chunk_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            // Untrusted input: must return Ok(valid prefix) or Err(Malformed),
            // never panic.
            let _ = decode_chunk(&bytes);
        }

        #[test]
        fn decode_chunk_never_panics_on_valid_header_plus_garbage(
            garbage in proptest::collection::vec(any::<u8>(), 0..128),
        ) {
            let mut v = encode_header(None).expect("header").to_vec();
            v.extend_from_slice(&garbage);
            let _ = decode_chunk(&v);
        }
    }

    #[test]
    fn body_with_unknown_extra_key_still_decodes_to_event() {
        // Forward-compat: a future encoder adds key 5 without bumping the
        // format version; this decoder must skip it.
        let mut body = Vec::new();
        {
            let mut e = minicbor::Encoder::new(&mut body);
            e.map(5).expect("map")
                .u32(0).expect("k0").u64(3).expect("v0")
                .u32(1).expect("k1").u32(1).expect("v1")
                .u32(2).expect("k2").str("E").expect("v2")
                .u32(4).expect("k4").bytes(b"data").expect("v4")
                .u32(5).expect("k5").u32(999).expect("v5"); // unknown key
        }
        let block = BlockRepr { crc: crc32c::crc32c(&body), body: &body };
        let mut chunk = encode_header(None).expect("header").to_vec();
        chunk.extend_from_slice(&encode_section_heading(b"s").expect("heading"));
        chunk.extend_from_slice(&minicbor::to_vec(&block).expect("block"));
        let sections = decode_chunk(&chunk).expect("decode");
        match &sections[0].blocks[0] {
            ImportBlock::Event(e) => {
                assert_eq!(e.version().as_u64(), 3);
                assert_eq!(e.payload(), b"data");
            }
            ImportBlock::Corrupt => panic!("unknown key must be skipped, not corrupt"),
        }
    }
```

- [ ] **Step 2: Run, verify pass.**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing cbor::tests`
Expected: PASS. If `body_with_unknown_extra_key_still_decodes_to_event` fails, minicbor's derived `Decode` is not skipping unknown keys — confirm the `#[cbor(map)]` derive (not `#[cbor(array)]`) is on `BodyRepr`.

- [ ] **Step 3: Run the full crate test + clippy gate locally.**

```bash
nix develop -c cargo nextest run -p nexus-store --features cbor,testing
nix develop -c cargo clippy -p nexus-store --features cbor,testing --all-targets
```

Expected: all green, zero clippy warnings. Fix any lint in non-test code (test modules carry the scoped `#[allow]`).

- [ ] **Step 4: Commit.**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/
git commit -m "$(cat <<'EOF'
test(store): cbor box defensive fuzz (never-panic) + forward-compat key skip

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Wait for the hook; confirm HEAD changed.

---

## Task 11: Open the PR

- [ ] **Step 1: Push and open the PR (stacked on Cards 0–2).**

```bash
git push -u origin feat/export-import-cbor-box
gh pr create --base feat/export-import-traits \
  --title "feat(store): default CBOR backup box (export/import Card 3, #145)" \
  --body "$(cat <<'EOF'
## Summary

Card 3 of export/import (#145): the default CBOR backup box — the bytes↔sections
travel codec between Card 1's raw export and Card 2's `EventImporter`. Stacked on
#218 (Cards 0–2); base will retarget to `main` once #218 merges.

- New `cbor` feature: `cbor = ["import", "export", "dep:minicbor", "dep:crc32c"]`
  (implies export+import; never the reverse — Agency brings its own CESR box).
- CBOR sequence (RFC 8742): header map → per-stream section heading map → block
  arrays `[crc32c, body]`. `stream_id` recorded once per section; `global_seq`
  never written (restamped on import).
- Encode = 3 stateless free functions returning appendable `Bytes`
  (`encode_header`/`encode_section_heading`/`encode_block`).
- Decode = `decode_chunk` (→ `Vec<StreamSection>`) + `decode_header` peek.
  Trust model: crc mismatch ⇒ `ImportBlock::Corrupt` (localized); truncation ⇒
  valid-prefix stop; any other structural violation ⇒ `ChunkError::Malformed`.

Design + plan: `docs/plans/2026-06-21-export-import-cbor-box-{design,plan}.md`.

## Tests

4 cross-cutting categories first (sequence/protocol, lifecycle/incremental,
defensive boundary, isolation), then insta golden bytes, a VOPR round-trip
proptest, a crc single-byte-mutation proptest, and never-panic fuzz +
forward-compat unknown-key skip.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Confirm the PR is open and CI is running.**

```bash
gh pr view --json number,baseRefName,state
```

---

## Self-review against the spec

- **CBOR layout (header map / heading map / block array; body keys 0–4; no `stream_id`; no `global_seq`):** Tasks 2 (`HeaderRepr`), 4 (`HeadingRepr`), 3 (`BodyRepr` keys 0–4, `BlockRepr` array). Golden bytes in Task 8 pin them.
- **Decode trust table (Corrupt only from crc mismatch; truncation ⇒ valid prefix; else Malformed):** `decode_block` + `decode_chunk` in Tasks 3–4; verified in Tasks 5 (Malformed/Corrupt), 6 (truncation), 10 (never-panic).
- **`encode_block` infallible → resolved to `Result`:** Task 3, error via `ChunkError::Encode(#[from] ...)` from Task 1.
- **API trio + `decode_chunk`/`decode_header`/`ChunkHeader`/`ChunkError`:** Tasks 1–4; re-exported in Task 1 Step 4.
- **`PersistedEnvelope` reconstruction via `try_new`, `global_seq = GlobalSeq::INITIAL`:** `reconstruct` in Task 3.
- **Feature wiring + self dev-dep + `cargo add`:** Task 1.
- **Forward-compat unknown-key skip:** Task 10.
- **Full pipeline export→box→import:** Task 7.

No placeholders; every code step shows full code; types/signatures are consistent across tasks (`ChunkError`, `ChunkHeader`, `HeaderRepr`/`HeadingRepr`/`BodyRepr`/`BlockRepr`, `decode_block`/`reconstruct`).
