# CBOR `ChunkWriter` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the CBOR backup box a typestate `ChunkWriter` write-path that mirrors `decode_chunk`'s elegance — no manual byte concatenation, no hand-ordered framing — and delete the three low-level `encode_*` functions it replaces.

**Architecture:** A `ChunkWriter<W>` over a `minicbor::Encoder<W>` writes the header in its constructor; `section(id)` returns a borrow-scoped `SectionWriter<'_, W>` that is the only type able to write blocks (block-before-heading is a compile error). No `finish()` — a CBOR sequence (RFC 8742) has no terminator. The writer is driven from the export stream via `SectionWriter::try_extend` + caller-side `TryStreamExt::try_fold`.

**Tech Stack:** Rust 2024, `minicbor` 2.x (`Encode` derive, `Encoder<W>`, `encode::Write`), `crc32c`, `thiserror`, `futures` (`Stream`/`StreamExt`), `proptest`, `insta`.

Spec: `docs/plans/2026-06-29-cbor-chunk-writer-design.md`.

---

## File Structure

- **Modify** `crates/nexus-store/src/cbor.rs` — add `WriteError`, `SectionError`, `ChunkWriter`, `SectionWriter`; delete `encode_header`/`encode_section_heading`/`encode_block`; strip `ChunkError::Encode`; migrate in-module tests. (All write-path code lives in this one file, matching the box's existing flat layout.)
- **Modify** `crates/nexus-store/src/lib.rs:130-134` — update the `#[cfg(feature = "cbor")]` re-export block.
- **Modify** `crates/nexus-fjall/tests/export_import_tests.rs` — migrate the `build_chunk` helper.
- **Modify** `examples/fjall-end-to-end/src/lib.rs` — migrate the `build_chunk` helper.

All work is on branch `feat/cbor-chunk-writer` (already created; design doc committed).

**Gate notes (from project memory):**
- The pre-commit hook runs `nix flake check` automatically on every commit — do NOT run it by hand first.
- Gate clippy is `--lib` only; after the test/example migrations (Tasks 7) run `nix develop -c cargo clippy --all-targets --all-features` by hand.
- Gate nextest runs default features only, but `nexus-store` has a self dev-dependency enabling `cbor`, so the in-module cbor tests ARE covered.

---

### Task 1: Write-path error types

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs` (imports near top; new types after `ChunkError`)

- [ ] **Step 1: Add the `Encoder` import**

In the `use minicbor::...` group at the top of `cbor.rs`, add `Encoder` alongside `Decoder`:

```rust
use minicbor::{Decoder, Encoder};
use minicbor::data::Type;
```

(Replace the existing `use minicbor::Decoder;` / `use minicbor::data::Type;` lines.)

- [ ] **Step 2: Add the failing test**

Add to the `mod tests` block in `cbor.rs`:

```rust
#[test]
fn write_error_displays_and_is_error() {
    // Infallible sink error: the WriteError type must still be constructible and
    // implement std::error::Error so it composes in caller error chains.
    fn assert_error<E: std::error::Error>() {}
    assert_error::<WriteError<core::convert::Infallible>>();
    assert_error::<SectionError<core::convert::Infallible, std::io::Error>>();
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::write_error_displays_and_is_error`
Expected: FAIL — `cannot find type WriteError`.

- [ ] **Step 4: Add the error types**

After the `ChunkError` enum in `cbor.rs`, add:

```rust
/// A write-path failure, generic over the sink's write error `E` (`W::Error`).
///
/// Distinct from [`ChunkError`] (the read domain, CLAUDE rule 3). Wraps
/// minicbor's encode error; unlike the old `Vec`-only encoders this can fire for
/// real (a generic sink — e.g. a file — can fail mid-write).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteError<E> {
    /// minicbor serialization or the sink failed while writing a chunk item.
    #[error("chunk write failed: {0}")]
    Encode(#[from] minicbor::encode::Error<E>),
}

/// A [`SectionWriter::try_extend`] failure: the two domains it spans, kept
/// distinct (CLAUDE rule 3) — a read failure is never reported as a write
/// failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SectionError<E, R> {
    /// Writing a block into the chunk failed.
    #[error(transparent)]
    Write(#[from] WriteError<E>),
    /// The source event stream yielded an error.
    #[error("event stream read failed: {0}")]
    Read(#[source] R),
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::write_error_displays_and_is_error`
Expected: PASS. (If the derive complains about missing bounds, add `where E: std::error::Error` style bounds the compiler names — but `minicbor::encode::Error<E>: Error` already gates this, so it should compile clean.)

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/cbor.rs
git commit -m "feat(store): add CBOR write-path error types (#246)"
```

---

### Task 2: `ChunkWriter::new` + `into_sink` (header only)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Add the failing test**

Add to `mod tests`:

```rust
#[test]
fn writer_header_decodes() {
    let mut w = ChunkWriter::new(Vec::new(), Some(b"dev-1")).expect("new");
    let bytes = Bytes::from(w.into_sink());
    let header = decode_header(&bytes).expect("decode header");
    assert_eq!(header.format_version, 1);
    assert_eq!(header.origin.as_deref(), Some(b"dev-1".as_slice()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::writer_header_decodes`
Expected: FAIL — `cannot find type ChunkWriter`.

- [ ] **Step 3: Implement `ChunkWriter::new` + `into_sink`**

Add to `cbor.rs` (after the `decode_header` function, before the `HeadingRepr` section):

```rust
/// A chunk being written. The header is already on the wire the moment this
/// exists — you cannot hold a `ChunkWriter` without it (Arrow's
/// schema-in-constructor discipline). There is deliberately no `finish`: a CBOR
/// sequence (RFC 8742) has no terminator, so a chunk is complete when writing
/// stops. Recover the buffer with [`ChunkWriter::into_sink`].
pub struct ChunkWriter<W> {
    enc: Encoder<W>,
}

impl<W: minicbor::encode::Write> ChunkWriter<W> {
    /// Construct a writer over `sink` and emit the chunk header (magic + format
    /// version + optional `origin` producer/device id).
    ///
    /// # Errors
    /// Returns [`WriteError`] if writing the header to the sink fails.
    pub fn new(sink: W, origin: Option<&[u8]>) -> Result<Self, WriteError<W::Error>> {
        let mut enc = Encoder::new(sink);
        enc.encode(HeaderRepr {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            origin,
        })?;
        Ok(Self { enc })
    }

    /// Recover the sink. NOT a format "finish" (a CBOR sequence has no
    /// terminator) — this just hands back everything written so far.
    pub fn into_sink(self) -> W {
        self.enc.into_writer()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::writer_header_decodes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/cbor.rs
git commit -m "feat(store): ChunkWriter::new emits the header in the constructor (#246)"
```

---

### Task 3: `section` + `SectionWriter::block`

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Add the failing test**

Add to `mod tests`:

```rust
#[test]
fn writer_multi_section_round_trips() {
    let mut w = ChunkWriter::new(Vec::new(), Some(b"dev")).expect("new");
    {
        let mut s = w.section(b"task-1").expect("section");
        s.block(&persisted(1, 1, "E", None, b"a1")).expect("block");
        s.block(&persisted(2, 1, "E", Some(b"m"), b"a2")).expect("block");
    }
    {
        let mut s = w.section(b"task-2").expect("section");
        s.block(&persisted(1, 2, "E", None, b"b1")).expect("block");
    }
    let chunk = Bytes::from(w.into_sink());

    let sections = decode_chunk(&chunk).expect("decode");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].origin.as_ref(), b"task-1");
    assert_eq!(sections[0].blocks.len(), 2);
    assert_eq!(sections[1].origin.as_ref(), b"task-2");
    match (&sections[0].blocks[1], &sections[1].blocks[0]) {
        (ImportBlock::Event(a2), ImportBlock::Event(b1)) => {
            assert_eq!(a2.metadata(), Some(b"m".as_slice()));
            assert_eq!(a2.payload(), b"a2");
            assert_eq!(b1.schema_version(), 2);
            assert_eq!(b1.payload(), b"b1");
        }
        _ => panic!("expected Event blocks"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::writer_multi_section_round_trips`
Expected: FAIL — `no method named section`.

- [ ] **Step 3: Implement `section` + `SectionWriter` + `block`**

Add the `section` method inside the existing `impl<W: minicbor::encode::Write> ChunkWriter<W>` block (after `new`, before `into_sink`):

```rust
    /// Begin a section for `stream_id`, recording the id once. Borrows
    /// `&mut self`, so the previous section's [`SectionWriter`] must be dropped
    /// first — sections cannot interleave.
    ///
    /// # Errors
    /// Returns [`WriteError`] if writing the heading to the sink fails.
    pub fn section(&mut self, stream_id: &[u8]) -> Result<SectionWriter<'_, W>, WriteError<W::Error>> {
        self.enc.encode(HeadingRepr { stream_id })?;
        Ok(SectionWriter { enc: &mut self.enc })
    }
```

Then add the `SectionWriter` type and its `block` method (after the `ChunkWriter` impl block).

**Background (verified against minicbor 2.x, `src/encode/error.rs`):** the block wire shape is `[crc32c(body), body]`, so the body must be serialized to a scratch buffer *before* the crc can be computed and written. `minicbor::to_vec(&body)` does that and returns `Result<Vec<u8>, minicbor::encode::Error<Infallible>>` (the `Vec` sink's write error is `Infallible`). That error type does not match `WriteError<W::Error>`, so it cannot flow through `?` directly. The clean widening: `minicbor::encode::Error<Infallible>` implements `Display` (its `E: Display` bound is met by `Infallible`), and `Error::message<T: Display>(msg: T)` builds an `Error<E>` for *any* `E` from a `Display` value — so `Error::message(e)` re-types the error while preserving its message. No `unwrap`/`expect`/`panic` (clippy-clean), and the path is unreachable in practice (the `Vec` sink never fails, and a `PersistedEnvelope` guarantees every field cap).

Add this free helper near the error types:

```rust
/// Widen a scratch-buffer (`Vec`) encode error into the writer's error domain.
/// The `Vec` sink's write error is `Infallible`, so this path is unreachable;
/// it only satisfies the type checker, preserving the error's message.
fn widen_encode_err<E>(
    e: minicbor::encode::Error<core::convert::Infallible>,
) -> WriteError<E> {
    WriteError::Encode(minicbor::encode::Error::message(e))
}
```

Then the `SectionWriter`:

```rust
/// A section whose heading is open. The ONLY type that can write blocks, so
/// "block before heading" is un-nameable — a compile error, not a runtime check.
/// Obtained from [`ChunkWriter::section`].
pub struct SectionWriter<'a, W> {
    enc: &'a mut Encoder<W>,
}

impl<W: minicbor::encode::Write> SectionWriter<'_, W> {
    /// Append one event as a block `[crc32c(body), body]`. The crc covers
    /// exactly the body bstr bytes.
    ///
    /// # Errors
    /// Returns [`WriteError`] if writing the block to the sink fails.
    pub fn block(&mut self, event: &PersistedEnvelope) -> Result<&mut Self, WriteError<W::Error>> {
        let body = BodyRepr {
            version: event.version().as_u64(),
            schema_version: event.schema_version(),
            event_type: event.event_type(),
            metadata: event.metadata(),
            payload: event.payload(),
        };
        // Scratch-encode the body so it can be CRC'd before it is written.
        let body_bytes = minicbor::to_vec(&body).map_err(widen_encode_err)?;
        self.enc.encode(BlockRepr {
            crc: crc32c::crc32c(&body_bytes),
            body: &body_bytes,
        })?;
        Ok(self)
    }
}
```

The main-sink `self.enc.encode(...)?` returns `minicbor::encode::Error<W::Error>`, which `WriteError<W::Error>` absorbs via its `#[from]`, so that `?` needs no widening — only the scratch `to_vec` does.

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::writer_multi_section_round_trips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/cbor.rs
git commit -m "feat(store): ChunkWriter::section + SectionWriter::block (#246)"
```

---

### Task 4: `SectionWriter::try_extend` (consume the export stream)

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`

- [ ] **Step 1: Add the failing tests**

Add to `mod tests` (the module already has `use futures::StreamExt;`; add `use futures::stream;` if missing — verify and add to the imports at top of `mod tests`):

```rust
#[tokio::test]
async fn try_extend_drains_stream_into_section() {
    let events = vec![
        Ok::<_, std::io::Error>(persisted(1, 1, "E", None, b"x1")),
        Ok(persisted(2, 1, "E", Some(b"m"), b"x2")),
    ];
    let mut w = ChunkWriter::new(Vec::new(), None).expect("new");
    w.section(b"s")
        .expect("section")
        .try_extend(futures::stream::iter(events))
        .await
        .expect("extend");
    let chunk = Bytes::from(w.into_sink());

    let sections = decode_chunk(&chunk).expect("decode");
    assert_eq!(sections[0].blocks.len(), 2);
    match &sections[0].blocks[1] {
        ImportBlock::Event(e) => assert_eq!(e.payload(), b"x2"),
        ImportBlock::Corrupt => panic!("expected Event"),
    }
}

#[tokio::test]
async fn try_extend_surfaces_read_error_distinctly() {
    let boom = std::io::Error::new(std::io::ErrorKind::Other, "boom");
    let events = vec![
        Ok(persisted(1, 1, "E", None, b"x1")),
        Err(boom),
    ];
    let mut w = ChunkWriter::new(Vec::new(), None).expect("new");
    let err = w
        .section(b"s")
        .expect("section")
        .try_extend(futures::stream::iter(events))
        .await
        .expect_err("read error propagates");
    match err {
        SectionError::Read(io) => assert_eq!(io.to_string(), "boom"),
        SectionError::Write(_) => panic!("a read failure must not be a write failure"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::try_extend`
Expected: FAIL — `no method named try_extend`.

- [ ] **Step 3: Implement `try_extend`**

Add to the top-of-file imports: ensure `use futures::Stream;` and `use futures::StreamExt;` are present (add to the existing `use futures...` line, or add new `use` lines in the top-of-file import block — NOT mid-file).

Add the method inside `impl<W: minicbor::encode::Write> SectionWriter<'_, W>` (after `block`):

```rust
    /// Drain a fallible event stream (e.g. an export stream) into this section,
    /// writing each event as a block. Mirrors `TryStreamExt::try_collect`.
    ///
    /// # Errors
    /// [`SectionError::Read`] if the source stream yields an error;
    /// [`SectionError::Write`] if writing a block to the sink fails. The two
    /// domains never collapse into one (CLAUDE rule 3).
    pub async fn try_extend<S, R>(&mut self, events: S) -> Result<&mut Self, SectionError<W::Error, R>>
    where
        S: Stream<Item = Result<PersistedEnvelope, R>>,
    {
        futures::pin_mut!(events);
        while let Some(item) = events.next().await {
            let env = item.map_err(SectionError::Read)?;
            self.block(&env).map_err(SectionError::Write)?;
        }
        Ok(self)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::tests::try_extend`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/cbor.rs
git commit -m "feat(store): SectionWriter::try_extend drains an export stream (#246)"
```

---

### Task 5: Migrate in-module tests onto `ChunkWriter`

This re-points every in-module test that builds a chunk to the writer, so Task 6 can delete the low-level encoders with nothing left referencing them.

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs` (test module only)

- [ ] **Step 1: Rewrite the private `encode_chunk` test helper**

Replace the existing `fn encode_chunk(...)` helper (currently the manual `Vec` + `extend_from_slice` loop) with:

```rust
    /// Build a full multi-stream chunk through the public `ChunkWriter`.
    fn encode_chunk(origin: Option<&[u8]>, streams: &[(&[u8], Vec<PersistedEnvelope>)]) -> Bytes {
        let mut w = ChunkWriter::new(Vec::new(), origin).expect("writer");
        for (stream_id, events) in streams {
            let mut s = w.section(stream_id).expect("section");
            for e in events {
                s.block(e).expect("block");
            }
        }
        Bytes::from(w.into_sink())
    }
```

- [ ] **Step 2: Rewrite the `prefix_len_after_n_blocks` helper**

Replace it with a writer-built variant (builds a fresh chunk with exactly `n` blocks and takes its length — a CBOR sequence prefix is deterministic, so this length is exactly the cut into the full chunk):

```rust
    /// Byte length of a chunk holding the header, one heading, and the first `n`
    /// blocks — i.e. the valid-prefix cut after `n` blocks in the full chunk.
    fn prefix_len_after_n_blocks(stream_id: &[u8], events: &[PersistedEnvelope], n: usize) -> usize {
        let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
        {
            let mut s = w.section(stream_id).expect("section");
            for e in events.iter().take(n) {
                s.block(e).expect("block");
            }
        }
        w.into_sink().len()
    }
```

- [ ] **Step 3: Replace `encode_section_heading` uses in defensive tests**

Three tests build `header + heading + crafted-bad-block` manually:
`crc_valid_but_body_invalid_is_malformed`, `body_with_unknown_extra_key_still_decodes_to_event`, and `crc_valid_body_with_empty_metadata_is_malformed`. In each, replace the two lines

```rust
        let mut chunk = encode_header(None).expect("header").to_vec();
        chunk.extend_from_slice(&encode_section_heading(b"s").expect("heading"));
```

with a writer-built header+heading prefix:

```rust
        let mut chunk = {
            let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
            w.section(b"s").expect("section");
            w.into_sink()
        };
```

Leave the subsequent `chunk.extend_from_slice(&minicbor::to_vec(&block)...)` lines (the crafted bad block) unchanged.

- [ ] **Step 4: Rewrite the full-pipeline test's chunk build**

In `export_box_import_round_trip_byte_equal_modulo_global_seq`, replace the manual build block (the `let mut chunk = encode_header(...)` through the nested `while let` read loop) with a writer + `try_extend`:

```rust
        let mut w = ChunkWriter::new(Vec::new(), Some(b"src")).expect("writer");
        for sid in ["task-1", "task-2"] {
            let s = src
                .read_stream(&StreamKey::from_slice(sid.as_bytes()), Version::INITIAL)
                .await
                .expect("read");
            w.section(sid.as_bytes())
                .expect("section")
                .try_extend(s)
                .await
                .expect("extend");
        }
        let chunk = w.into_sink();
```

(`read_stream` yields `Result<PersistedEnvelope, _>` items, so `try_extend` consumes it directly.)

- [ ] **Step 5: Re-point the golden-byte snapshots**

`golden_header_bytes` and `golden_block_bytes` currently call `encode_header`/`encode_block`. Re-point them to the writer so they assert the wire format did not move:

```rust
    #[test]
    fn golden_header_bytes() {
        let w = ChunkWriter::new(Vec::new(), Some(b"dev")).expect("writer");
        insta::assert_snapshot!("header_with_origin_hex", hex_of(&w.into_sink()));
    }

    #[test]
    fn golden_block_bytes() {
        // Build header + heading + one block, then strip the header+heading
        // prefix so the snapshot is the block bytes alone — byte-identical to the
        // old encode_block output (same BlockRepr).
        let full = {
            let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
            let mut s = w.section(b"x").expect("section");
            s.block(&persisted(1, 1, "E", None, b"hi")).expect("block");
            w.into_sink()
        };
        let head_heading = {
            let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
            w.section(b"x").expect("section");
            w.into_sink()
        };
        let block_only = &full[head_heading.len()..];
        insta::assert_snapshot!("block_v1_hex", hex_of(block_only));
    }
```

VERIFY the existing snapshot files in `crates/nexus-store/src/snapshots/` still match. The block bytes are byte-identical to the old `encode_block` output (same `BlockRepr`), so `block_v1_hex` is unchanged; `header_with_origin_hex` is also unchanged (same `HeaderRepr`). If `cargo insta` reports a diff, the format moved — investigate before accepting.

- [ ] **Step 6: Re-point the VOPR property test**

In `vopr_chunk_round_trips`, the chunk is already built via the private `encode_chunk` helper (rewritten in Step 1), so no change is needed there — confirm it still calls `encode_chunk(origin.as_deref(), &refs)`.

- [ ] **Step 7: Run the full cbor test module**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::`
Expected: PASS (all). If insta snapshots changed unexpectedly, STOP and investigate.

- [ ] **Step 8: Commit**

```bash
git add crates/nexus-store/src/cbor.rs
git commit -m "test(store): drive cbor box tests through ChunkWriter (#246)"
```

---

### Task 6: Delete the low-level encoders + strip `ChunkError::Encode`

**Files:**
- Modify: `crates/nexus-store/src/cbor.rs`
- Modify: `crates/nexus-store/src/lib.rs:130-134`

- [ ] **Step 1: Delete the three `encode_*` functions**

Remove `pub fn encode_header(...)`, `pub fn encode_section_heading(...)`, and `pub fn encode_block(...)` from `cbor.rs` (their doc comments too). Keep `HeaderRepr`, `HeadingRepr`, `BodyRepr`, `BlockRepr` (now used by the writer and the decoder).

- [ ] **Step 2: Strip `ChunkError::Encode` and the now-unused `Infallible` import**

Change `ChunkError` to:

```rust
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChunkError {
    /// Decode: the chunk framing is unreadable. The `&'static str` is a debug
    /// hint. Distinct from a per-block crc failure, which is a non-error
    /// [`ImportBlock::Corrupt`](crate::import::ImportBlock::Corrupt).
    #[error("malformed chunk: {0}")]
    Malformed(&'static str),
}
```

Remove `use core::convert::Infallible;` from the top of `cbor.rs` **only if** no other code references `Infallible` (the `rewrap_encode_err` helper from Task 3 uses `core::convert::Infallible` inline — keep that import if so, or keep the inline path). Let the compiler tell you: an unused-import warning is denied by clippy, so resolve it whichever way compiles clean.

- [ ] **Step 3: Update `lib.rs` re-exports**

Replace `crates/nexus-store/src/lib.rs:130-134`:

```rust
#[cfg(feature = "cbor")]
pub use cbor::{
    ChunkError, ChunkHeader, ChunkWriter, SectionError, SectionWriter, WriteError, decode_chunk,
    decode_header,
};
```

- [ ] **Step 4: Run cbor tests + lib build**

Run: `nix develop -c cargo test -p nexus-store --features cbor,testing,export,import -- cbor::`
Expected: PASS. No references to the deleted functions remain.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/cbor.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store)!: delete low-level CBOR encoders; ChunkWriter is the write path (#246)"
```

---

### Task 7: Migrate the fjall test + example `build_chunk`

**Files:**
- Modify: `crates/nexus-fjall/tests/export_import_tests.rs:33-35` (imports) and `:87-109` (`build_chunk`)
- Modify: `examples/fjall-end-to-end/src/lib.rs:68` (import) and `:287-307` (`build_chunk`)

- [ ] **Step 1: Migrate the fjall test helper**

In `crates/nexus-fjall/tests/export_import_tests.rs`, update the import (line ~33) from
`ChunkError, decode_chunk, encode_block, encode_header, encode_section_heading,`
to:

```rust
    ChunkError, ChunkWriter, decode_chunk,
```

Replace `build_chunk` (lines ~87-109) with the `try_fold`-free explicit form (the test already has `ids` sorted; keep that), driving the writer:

```rust
async fn build_chunk(store: &FjallStore) -> Vec<u8> {
    let mut ids: Vec<Vec<u8>> = store
        .list_streams()
        .await
        .expect("list opens")
        .map(|r| r.expect("no list error").as_bytes().to_vec())
        .collect::<Vec<_>>()
        .await;
    ids.sort();

    let mut w = ChunkWriter::new(Vec::new(), None).expect("writer");
    for id_bytes in ids {
        let id = identity_route(&id_bytes);
        let events = store
            .read_stream(&id, Version::INITIAL)
            .await
            .expect("read opens");
        w.section(&id_bytes)
            .expect("section")
            .try_extend(events)
            .await
            .expect("extend");
    }
    w.into_sink()
}
```

VERIFY the imports the new body needs (`Version`, `StreamExt` for `list_streams`’ `.map`/`.collect`) are already present in the test file (they are used by neighbouring helpers). Add any missing `use` at the top only.

- [ ] **Step 2: Migrate the example helper (the user-facing proof)**

In `examples/fjall-end-to-end/src/lib.rs`, update the import (line ~68) from
`use nexus_store::cbor::{decode_chunk, encode_block, encode_header, encode_section_heading};`
to:

```rust
use nexus_store::cbor::{decode_chunk, ChunkWriter};
```

Replace `build_chunk` (lines ~287-307) with the `try_fold` pipeline — the showcase form:

```rust
/// Back up every stream through the CBOR box, driving `ChunkWriter` from the
/// export streams with `TryStreamExt::try_fold` — one functional pipeline, no
/// manual byte concatenation, no hand-ordered framing (#246).
async fn build_chunk(store: &Store<FjallStore>) -> Result<Vec<u8>, BoxErr> {
    let mut ids: Vec<StreamKey> = store
        .list_streams()
        .await?
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
    ids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let writer = futures::stream::iter(ids.into_iter().map(Ok::<_, BoxErr>))
        .try_fold(ChunkWriter::new(Vec::new(), None)?, |mut w, id| async move {
            let events = store.export_stream(&id, Version::INITIAL).await?;
            w.section(id.as_bytes())?.try_extend(events).await?;
            Ok(w)
        })
        .await?;
    Ok(writer.into_sink())
}
```

VERIFY `BoxErr` (the example's boxed error) absorbs `WriteError<Infallible>` and `SectionError<Infallible, FjallError>` via `?` — both are `std::error::Error + Send + Sync + 'static`, so `Box<dyn Error + ...>` accepts them. Ensure `use futures::TryStreamExt;` is imported (add at top if absent). If `try_fold`’s closure-capture of `store` fights the borrow checker, fall back to the explicit `for id in ids { ... }` loop (same body), which is equally valid — note this is acceptable.

- [ ] **Step 3: Build + test fjall and the example**

Run: `nix develop -c cargo test -p nexus-fjall --features export,import,cbor,snapshot -- export_import`
Expected: PASS.

Run: `nix develop -c cargo build -p fjall-end-to-end` (or the example's actual package name — confirm via `examples/fjall-end-to-end/Cargo.toml`).
Expected: builds clean.

- [ ] **Step 4: Clippy the non-lib targets by hand (gate is --lib only)**

Run: `nix develop -c cargo clippy --all-targets --all-features`
Expected: no warnings (clippy is `-D warnings` in this workspace).

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/tests/export_import_tests.rs examples/fjall-end-to-end/src/lib.rs
git commit -m "refactor(fjall,examples): collapse build_chunk onto ChunkWriter (#246)"
```

(The pre-commit hook runs `nix flake check`; let it.)

---

### Task 8: Update CLAUDE.md architecture note

**Files:**
- Modify: `CLAUDE.md` (the `cbor.rs` bullet under the Store Crate section)

- [ ] **Step 1: Update the `cbor.rs` description**

Find the `cbor.rs` bullet and append a sentence documenting the write path:

> The write path is the typestate `ChunkWriter<W>` (header emitted in the constructor; `section(id) -> SectionWriter` whose `block`/`try_extend` are the only way to write blocks, making block-before-heading a compile error; no `finish` — a CBOR sequence has no terminator; `into_sink()` recovers the buffer). Driven from export streams via `TryStreamExt::try_fold`. The old low-level `encode_header`/`encode_section_heading`/`encode_block` were deleted (#246); read stays the asymmetric `decode_header`/`decode_chunk` because decode is handed the whole buffer.

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the ChunkWriter write path in CLAUDE.md (#246)"
```

---

## Self-Review

**Spec coverage:**
- `ChunkWriter<W>` / `new` / `section` / `into_sink` → Tasks 2, 3. ✓
- `SectionWriter` / `block` / `try_extend` → Tasks 3, 4. ✓
- No `finish` (RFC 8742) → encoded in design + doc comments, Task 2. ✓
- Generic sink → `W: minicbor::encode::Write`, Task 2; `Vec<u8>` baseline used throughout. ✓
- `try_fold` driver → Task 7 example. ✓
- `WriteError` / `SectionError` distinct domains → Task 1. ✓
- `ChunkError::Encode` removed → Task 6. ✓
- Delete `encode_*` → Task 6. ✓
- Migrations (fjall test, example, in-module tests) → Tasks 5, 7. ✓
- Four testing categories → sequence (Task 3), lifecycle (existing file/torn-prefix tests, Task 5 keeps them), defensive boundary (existing band tests, Task 5 keeps them), linearizability N/A (single-owner encoder). ✓

**Open verification items flagged for the implementer (do NOT assume — check the crate):**
1. `widen_encode_err` (Task 3) uses `minicbor::encode::Error::message(e)` — verified present in minicbor 2.x (`src/encode/error.rs`: `message<T: Display>` under `alloc`, and `Error<Infallible>: Display`). Confirm it still compiles; this is the one genuinely fiddly spot.
2. `golden_block_bytes` snapshot must remain byte-identical (Task 5 Step 5).
3. `BoxErr`/`?` absorption of the new error types in the example (Task 7 Step 2).

**Type consistency:** `ChunkWriter`, `SectionWriter`, `WriteError`, `SectionError`, `section`, `block`, `try_extend`, `into_sink`, `widen_encode_err` used identically across all tasks. ✓

**Placeholder scan:** none. The Task 3 body-encode path is a single verified implementation (`widen_encode_err`); earlier dead-end exploration was removed.
