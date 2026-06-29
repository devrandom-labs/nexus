# CBOR backup box — the symmetric write-path (`ChunkWriter`) — design

Issue #246, milestone 1 (Pre-Freeze, 1.0 blockers). Surfaced by #227. A research
+ implementation card: give the CBOR backup box (`cbor.rs`) a write-path that is
as elegant and misuse-resistant as its read-path (`decode_chunk`), so building a
chunk needs **no manual byte concatenation and no hand-ordered framing**.

## The problem (recap)

The box is asymmetric. Read side is one call:

```rust
let sections = decode_chunk(&bytes)?;            // Vec<StreamSection>, done
```

Write side is a hand-rolled, order-sensitive protocol — three low-level encoders
(`encode_header` / `encode_section_heading` / `encode_block`) the caller must
concatenate in exactly the right order, allocating a `Bytes` per piece:

```rust
let mut buf = Vec::new();
buf.extend_from_slice(&encode_header(None)?);
for id in ids {
    buf.extend_from_slice(&encode_section_heading(id.as_bytes())?);  // MUST precede its blocks
    for ev in stream { buf.extend_from_slice(&encode_block(&ev)?); } // in version order
}
```

A wrong order silently produces a chunk `decode_chunk` rejects or misreads. #227
grew a `build_chunk` helper to contain it; the fjall test and the
`fjall-end-to-end` example each carry a copy.

## Why the literal mirror is wrong

The obvious "symmetric" fix — `encode_chunk(header, &[StreamSection]) -> Bytes`
mirroring `decode_chunk` — is the trap:

- `StreamSection` carries `Vec<ImportBlock>` — already-**decoded** events,
  including `Corrupt`. It is the *import* type; reusing it on the write side is
  the wrong altitude (one never *exports* a `Corrupt` block).
- It forces every event of every stream resident before encoding, discarding the
  streaming export the whole `export.rs` layer is built around (the same
  memory-bound discipline `BatchSize` exists for; load-bearing on the IoT/mobile
  target).

`decode_chunk` gets to be one eager call only because decode is handed the whole
buffer. Encode is not: its source is a lazy `Stream`.

## Prior art (cited)

- **Apache Arrow IPC `StreamWriter`** writes the schema header *inside the
  constructor* (`try_new` emits the schema before returning), making
  header-before-records structurally impossible to violate; `write()` is
  incremental. — `https://github.com/apache/arrow-rs/blob/master/arrow-ipc/src/writer.rs`
- **Rust typestate** (consuming `self`, phantom/borrow states) is the only pattern
  that makes out-of-order calls a *compile* error — the method "simply isn't
  defined" on the wrong state. — `https://cliffle.com/blog/rust-typestate/`,
  `https://docs.rust-embedded.org/book/static-guarantees/typestate-programming.html`
- **RFC 8742 (CBOR Sequences):** "any number of encoded CBOR data items, simply
  concatenated — no end marker, no enclosing array." So this format has **no
  `finish()`** — a chunk is done when the bytes stop, and a heading simply starts
  a new section. The one guarantee typestate cannot give (you *must* call finish)
  does not exist here. — `https://www.rfc-editor.org/rfc/rfc8742.html`
- **minicbor `Encoder<W>`** borrows `&mut self` per `encode` and returns the sink
  via `into_writer()`; repeated `encode` *is* an RFC 8742 sequence, with no
  per-item allocation. `Write` is implemented for `Vec<u8>`, and a `Writer<W>`
  adapter bridges any `std::io::Write` (minicbor `std` feature). —
  `https://docs.rs/minicbor/latest/minicbor/encode/struct.Encoder.html`
- **`try_fold`** is the consume-a-stream-into-an-accumulator combinator (`scan` is
  a stateful *map* that produces a stream — wrong tool). The nexus repository
  facades already drive event-stream consumption via
  `futures::TryStreamExt::try_fold`.

## Design — `ChunkWriter<W>` + `SectionWriter<'a, W>`

A typestate writer over an output sink, fusing the three patterns above. Lives in
`cbor.rs`, gated `#[cfg(feature = "cbor")]`.

```rust
/// A chunk being written. The header is already on the wire — you cannot hold a
/// `ChunkWriter` without it (Arrow's schema-in-constructor move).
pub struct ChunkWriter<W> { /* enc: minicbor::Encoder<W> */ }

/// A section whose heading is open. The ONLY type that can write blocks.
pub struct SectionWriter<'a, W> { /* enc: &'a mut minicbor::Encoder<W> */ }

impl<W: minicbor::encode::Write> ChunkWriter<W> {
    /// Construct and emit the header (magic + format version + optional origin).
    pub fn new(sink: W, origin: Option<&[u8]>) -> Result<Self, WriteError<W::Error>>;

    /// Begin a section, recording the stream id once. Borrows `&mut self`, so the
    /// previous `SectionWriter` borrow must end first — sections cannot interleave.
    pub fn section(&mut self, stream_id: &[u8]) -> Result<SectionWriter<'_, W>, WriteError<W::Error>>;

    /// Recover the sink. NOT a format "finish" — a CBOR sequence has no terminator
    /// (RFC 8742); this just hands back what was written.
    pub fn into_sink(self) -> W;
}

impl<'a, W: minicbor::encode::Write> SectionWriter<'a, W> {
    /// Append one event as a block `[crc32c(body), body]`.
    pub fn block(&mut self, event: &PersistedEnvelope) -> Result<&mut Self, WriteError<W::Error>>;

    /// Drain a fallible event stream (the export stream) into this section.
    /// Mirrors `TryStreamExt::try_collect`/`try_fold`. Read errors and write
    /// errors are distinct domains (CLAUDE rule 3).
    pub async fn try_extend<S, E>(&mut self, events: S)
        -> Result<&mut Self, SectionError<W::Error, E>>
    where S: Stream<Item = Result<PersistedEnvelope, E>>;
}
```

### Misuse-resistance, by construction

- `block` / `try_extend` exist **only** on `SectionWriter`, and the only way to
  get a `SectionWriter` is `chunk.section(id)`. "Block before heading" is
  *un-nameable* — a compile error, not a runtime check.
- The header is emitted in `new` — it cannot be forgotten or misordered.
- `section` borrows `&mut self`, so a new section cannot start while the previous
  `SectionWriter` is alive — sections cannot interleave.
- No `finish()` to forget (RFC 8742). `into_sink()` only matters if you want the
  buffer back; forgetting it loses nothing already written.

### Block encoding (unchanged semantics)

`block` encodes the `BodyRepr` map to a small per-event scratch buffer, computes
`crc32c` over those body bytes, then writes the `[crc, body]` array into the live
`Encoder`. The per-event scratch is bounded by one event; no whole-chunk buffer
is allocated beyond the sink itself.

### The sink

Generic `W: minicbor::encode::Write`. `Vec<u8>` is the baseline in-memory sink
(alloc, already enabled) and preserves today's "return `Bytes`" ergonomics via
`Bytes::from(writer.into_sink())`. A file-backed sink (true output streaming, so
the whole serialized backup never sits in RAM) is available through minicbor's
`Writer<W>` adapter and would require enabling minicbor's `std` feature — left as
an extension, not wired up in this card (keeps the dependency surface flat at
freeze). The abstraction is correct either way.

## Driving the writer — `try_fold`, not a loop

The writer is a set of primitives; the *driving* is the caller's functional
pipeline, which keeps the box decoupled from any store (the box is pluggable —
CESR is the planned alternative — and the export→box→upload orchestration belongs
to Agency, not nexus). The whole backup becomes one pipeline:

```rust
let w = store.list_streams().await?
    .try_fold(ChunkWriter::new(Vec::new(), origin)?, |mut w, id| async move {
        let events = store.export_stream(&id, Version::INITIAL).await?;
        w.section(id.as_bytes())?.try_extend(events).await?;   // borrow ends here
        Ok(w)
    })
    .await?;
let chunk = Bytes::from(w.into_sink());
```

The only irreducible structure is "walk every stream", which is the caller's
domain by design. #227's `build_chunk` collapses to this.

## Errors

Write and read are distinct domains (CLAUDE rule 3); they get distinct types.

- `WriteError<E>` — the write-path failure, generic over the sink's write error
  `E` (`W::Error`). Wraps `minicbor::encode::Error<E>`. Real now (a generic sink
  can fail mid-write, e.g. a full disk), unlike the old "never fires" `Vec`-only
  path. A public **error** enum → carries `#[non_exhaustive]` at the freeze
  (CLAUDE rule 3 carve-out).
- `SectionError<E, R>` — `try_extend`'s two arms: `Write(WriteError<E>)` and
  `Read(R)` (the export stream's error). Never collapses a read failure into a
  write failure.
- `ChunkError` (read side) **loses its `Encode` variant** and becomes
  `Malformed`-only. That variant existed solely for the low-level `encode_*`
  functions; with those gone and the encode domain moved to `WriteError`, decode
  never encodes, so the variant is dead. Read = `ChunkError::Malformed`, write =
  `WriteError` / `SectionError` — a clean domain split.

> Dependency-seal note (#208): wrapping `minicbor::encode::Error<E>` names
> minicbor in the `WriteError` `From`/`source`. The existing `ChunkError::Encode`
> already accepts this for `Infallible`; this card keeps the same posture and
> defers any deeper seal to the #208 sweep rather than inventing a bespoke
> sealed error here.

## Public surface — delete the low-level encoders

Once `ChunkWriter` exists, the three low-level encoders have no remaining
callers: the writer absorbs their logic, and the box's own tests migrate to it
(the defensive tests that craft *invalid* bytes already do so via a raw
`minicbor::Encoder` against the `pub(crate)` `*Repr` structs, not via
`encode_block`). Keeping them would be dead code (CLAUDE: no gratuitous code), so
heading into the 1.0 freeze:

- **Delete** `encode_header` — logic moves into `ChunkWriter::new`.
- **Delete** `encode_section_heading` — logic moves into `ChunkWriter::section`.
- **Delete** `encode_block` — body-encode + crc logic moves into
  `SectionWriter::block`.
- **Keep** the `pub(crate)` wire-shape structs (`HeaderRepr`, `HeadingRepr`,
  `BodyRepr`, `BlockRepr`) — used by both the writer and the decoder.
- **Keep** `decode_header` + `decode_chunk` public. The read side stays
  asymmetric and that is justified: decode is handed the whole buffer; encode is
  driven from a stream.

`lib.rs` re-exports lose `encode_header` / `encode_section_heading` /
`encode_block`; they gain `ChunkWriter`, `SectionWriter`, `WriteError`,
`SectionError`.

## Migration

- `crates/nexus-fjall/tests/export_import_tests.rs::build_chunk` → the `try_fold`
  pipeline (or an explicit loop over `section().try_extend()` if the test wants
  to stay imperative).
- `examples/fjall-end-to-end/src/lib.rs::build_chunk` → the same; this is the
  user-facing proof that the helper collapsed.
- `cbor.rs` tests' private `encode_chunk` helper → built on `ChunkWriter`.

## Testing (CLAUDE rule 7 — four categories first)

The SUT is a pure encoder; it pairs with `decode_chunk` for round-trips.

1. **Sequence / protocol** — multi-section chunk via `ChunkWriter` decodes (via
   `decode_chunk`) to the exact sections written, in order; empty section
   followed by a real one; a section with zero blocks; header-only chunk decodes
   to an empty `Vec`. The compile-time guarantee (block-before-heading
   un-nameable) holds **by construction** — no runtime test can reach the illegal
   state because the method does not exist on `ChunkWriter`; assert this in a doc
   comment rather than adding a `trybuild` dev-dep at freeze.
2. **Lifecycle** — write a chunk to a real file, reopen, `decode_chunk`
   round-trips byte-for-byte modulo `global_seq`; torn-tail prefixes (cut after
   each block boundary) decode to the valid prefix (reuses the existing
   prefix-validity assertions, now produced by the writer).
3. **Defensive boundary** — feed the writer the field-boundary events
   (`event_type` / metadata / payload across CBOR length bands: inline, 1-byte,
   2-byte, max) and confirm `decode_chunk` recovers them exactly; oversize/zero
   fields are unrepresentable upstream (`PersistedEnvelope` guarantees), so the
   writer cannot emit them.
4. **Linearizability / isolation** — N/A for a pure single-owner encoder (no
   shared state, no concurrent writers to one `ChunkWriter`); stated explicitly
   rather than faked.

Plus: the existing VOPR `vopr_chunk_round_trips` property test re-pointed to build
its chunk through `ChunkWriter` (proves the writer's output is decode-equivalent
to the hand-assembled bytes for arbitrary multi-stream inputs); the
single-byte-mutation `Corrupt` property test is unchanged (it targets
`decode_block`); golden-byte insta snapshots re-pointed to the writer and
asserted byte-identical to the pre-refactor encoders (proves the wire format did
not move).

## Out of scope

- A store-aware `backup(store)` one-call API (couples the box to `RawEventStore`;
  belongs to Agency's sync runtime).
- Wiring minicbor's `std` feature for file sinks (separate, optional).
- Any change to the wire format, the chunk layout, or the read path.
