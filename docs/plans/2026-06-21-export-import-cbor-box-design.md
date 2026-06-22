# Export/Import Card 3 — the default CBOR backup box (design)

Issue #145, milestone 1 (Feature-Complete, pre-freeze). Branch
`feat/export-import-cbor-box`, stacked on `feat/export-import-traits` (PR #218,
Cards 0–2). This is the **last nexus-side card** for #145; the sync runtime
(per-device layout, iCloud transport, CRDT merge, the forever
export→upload→download→import loop) is Agency's (devrandom-labs/agency#147).

## What this card is

The bytes↔sections codec that sits between Card 1's raw export and Card 2's
`EventImporter`. It is the **default backup box** — the travel format. It does
two things:

- **Encode** (write side): turn a header + per-stream `PersistedEnvelope`s into
  appendable `Bytes` the caller writes to a chunk file incrementally.
- **Decode** (read side): turn chunk bytes back into `Vec<StreamSection>` for
  `EventImporter::import`, plus a cheap header peek.

It lives in a new `cbor.rs`, gated `#[cfg(feature = "cbor")]`, where
`cbor = ["import", "export", "dep:minicbor", "dep:crc32c"]`. `cbor` implies
export+import; export/import never imply `cbor` (Agency enables export/import and
brings its own CESR box, pulling no CBOR).

## Glossary (pinned — easy to blur)

- **frame** — one event as stored locally (`wire::encode_frame`, unchanged by
  this card). A book on the shelf. NOT what we build here.
- **block** — one event wrapped for the trip: `[crc32c, body]`. A mailed book
  with a fragile sticker.
- **section** — one origin stream's blocks, introduced by a heading that names
  the stream once.
- **chunk** — the travel box: one byte buffer holding a header + many sections.
- **box** — the codec in this card that turns events ↔ chunk bytes.

## Two layers, kept separate

Layer 1 = the on-disk wire frame (`wire.rs`), unchanged. Layer 2 = the export
**chunk** (this card). The label and per-block checksum go on the box, not in
every book. The chunk copies on decode — it carries **no** 16-byte alignment
guarantee (that is a frame-only concern for zero-copy local reads). Import is
not a hot path, so a copy/parse on decode is acceptable.

## The CBOR sequence layout (re-spec of §6 for per-stream sections)

The pre-pivot §6 put `stream_id` in every block body. The Card-2 pivot moved
`stream_id` to a per-stream **section heading**, emitted once, and dropped it
from block bodies. This is the re-spec.

A chunk is a **CBOR sequence** (RFC 8742) — a bare concatenation of CBOR data
items, no outer array. Definite-length encoding only; unsigned-integer map keys.

```
item[0]  HEADER   map   {0: magic bstr h'6e786368' ("nxch"),
                         1: format_version uint = 1,
                         2: origin bstr OPTIONAL}              ← chunk-level producer id
item[1]  HEADING  map   {0: stream_id bstr}                   ← starts a StreamSection
item[2]  BLOCK    array [crc32c uint, body bstr]              ← a block for the current section
item[3]  BLOCK    array [crc32c uint, body bstr]
item[4]  HEADING  map   {0: stream_id bstr}                   ← next StreamSection
item[5]  BLOCK    array [...]
...

body bstr = CBOR map {0: version uint ≥ 1,
                      1: schema_version uint ≥ 1,
                      2: event_type tstr (≤ MAX_EVENT_TYPE_LEN),
                      3: metadata bstr OPTIONAL (≤ MAX_METADATA_LEN, omitted when None),
                      4: payload bstr}
crc32c = CRC32C(body bstr bytes)   ; Castagnoli poly 0x1EDC6F41, the `crc32c` crate
```

### Why these shapes

- **Header / heading both maps; block an array.** The decoder distinguishes
  heading from block by peeking the next CBOR major type: **Map ⇒ heading**,
  **Array ⇒ block**. The header is also a map but is resolved positionally
  (always `item[0]`, read first and unconditionally). A map heading
  (`{0: stream_id}`) rather than a bare byte-string is chosen for
  forward-compatibility: integer keys are append-only, so a future per-section
  field can be added without a format break.
- **crc is element[0]** of the block array, so the reader has the expected
  checksum before reading the body. crc covers **exactly** the body bstr span —
  not the array, not itself.
- **`global_seq` is never written.** It is store-local; import restamps a fresh
  one on append (Card 2 confirmed import ignores it). Writing it would be work
  import redoes for free.
- **Block body keys renumbered 0–4** (was 0–5 with `stream_id` at 0). Allowed
  now, pre-freeze. After this card ships the keys are **append-only**: decoders
  skip unknown keys (forward-compat), and an incompatible change bumps
  `format_version` (an unknown version ⇒ `Malformed`).
- **No event-count, no end-marker.** Either would break incremental/live append.
  A chunk is "the valid blocks you can read"; truncation = fewer blocks.

## Trust model and the decode walk

Two trust levels, one clean rule: **bit-rot is localized (Corrupt);
structural violations reject the chunk (Malformed).** The only thing that
produces a `Corrupt` block is a crc mismatch. A passing crc means the body bytes
are intact-as-written, so any failure to interpret them is a format violation,
not corruption.

`decode_chunk(&[u8]) -> Result<Vec<StreamSection>, ChunkError>`:

| Situation | Outcome |
|---|---|
| header: bad magic / unknown `format_version` / not-a-map / truncated | `Malformed` |
| peek = end-of-input | **stop**, return accumulated sections (valid prefix) |
| peek = Map → heading `{0: stream_id}` decodes | start a new `StreamSection` |
| heading torn mid-item (end-of-input) | **stop** (valid prefix) |
| heading present but malformed (not the expected map) | `Malformed` |
| peek = Array → block `[crc, body]` torn mid-item (end-of-input) | **stop** (valid prefix) |
| block fully read, **crc mismatch** | `ImportBlock::Corrupt`, **continue** |
| block fully read, **crc match, body decodes** | `ImportBlock::Event`, continue (unknown body keys skipped) |
| block fully read, **crc match, body fails** (v0, schema 0, bad utf8, oversize, missing required key, not-a-map) | `Malformed` |
| block before any heading | `Malformed` |
| peek = any other major type | `Malformed` |

**Why end-of-input is the only "stop":** CBOR sequences are not
self-synchronizing — a corrupted length prefix makes the next item boundary
unfindable. End-of-input (truncation / torn tail) is the one non-clean
termination we can recover from, because everything before it is intact and
crc-checked per block. Any other parse failure past the header means the framing
is damaged in a way we cannot walk; returning a "valid prefix" there would risk
silently dropping present-but-unreachable data. `Malformed` is the honest
answer — the consumer re-fetches the chunk.

This never panics on arbitrary input: every failure is either a `Malformed`
result or a clean stop.

## Reconstructing `PersistedEnvelope` on decode

For each `Event` block, rebuild a `PersistedEnvelope` via
`PersistedEnvelope::try_new`:

- synthesize a backing `Bytes` buffer laid out `[event_type | metadata? | payload]`,
- compute the `event_type` / `metadata?` / `payload` `Range<u32>` offsets into it,
- `version` from body key 0, `schema_version` from body key 1,
- `global_seq = GlobalSeq::INITIAL` (a placeholder — import ignores it).

`try_new` re-validates ranges, UTF-8, and per-field caps at the box boundary
(each crate defends itself — CLAUDE rule). The box copies; no alignment needed.

## API surface

All in `cbor.rs`, behind `#[cfg(feature = "cbor")]`, re-exported at the crate
root.

**Encode — three free functions** (stateless, each returns a small `Bytes` the
caller appends; the encoder never owns a buffer or file IO, so an all-day
incremental append stays bounded in memory):

```rust
pub fn encode_header(origin: Option<&[u8]>) -> Result<Bytes, ChunkError>;
pub fn encode_section_heading(stream_id: &[u8]) -> Result<Bytes, ChunkError>;
pub fn encode_block(event: &PersistedEnvelope) -> Result<Bytes, ChunkError>;
```

**Resolved (from minicbor 2.2.2 source):** encode is fallible at the type level.
`minicbor::to_vec` returns `Result<Vec<u8>, encode::Error<Infallible>>`, and
`encode::Error` carries `Message` / `Custom` variants beyond the (uninhabited)
`Write(Infallible)` — so the compiler will not let us treat encoding as
infallible without `unwrap`/`expect`/`panic`, all of which strict clippy bans in
production code. The encode functions therefore return `Result<Bytes,
ChunkError>`, propagating the minicbor error via `?`. This also matches the
crate's existing fallible builder `wire::build_row`. In practice the error never
fires (a `PersistedEnvelope` guarantees every field cap, and the `Vec` writer is
`Infallible`), but the type is honest.

The caller's loop (Agency): `encode_header` once, then per stream
`encode_section_heading` followed by one `encode_block` per exported event,
appending each `Bytes` to the chunk file. Resume = re-open `export_stream(id,
from)` and continue.

**Decode:**

```rust
pub fn decode_chunk(bytes: &[u8]) -> Result<Vec<StreamSection>, ChunkError>;
pub fn decode_header(bytes: &[u8]) -> Result<ChunkHeader, ChunkError>;

pub struct ChunkHeader {
    pub format_version: u32,
    pub origin: Option<Bytes>,
}
```

`decode_header` is the cheap peek: validate magic + `format_version`, return the
chunk-level `origin` (device id) without parsing the body. It exists so the
chunk-level `origin` we encode is actually readable (a write-only field would
violate "justify every type") and so Agency can attribute a chunk to a device
before deciding the `route` closure. `decode_chunk` returns `Vec<StreamSection>`
to match `EventImporter::import(&[StreamSection], …)` — materialized because
import takes a slice and is not the hot path; a lazy streaming decoder is a
possible follow-up, out of scope here.

**The decode/encode asymmetry is intentional:** encode is incremental
(bounded-memory all-day append on the write side), decode is whole-buffer
(import consumes a slice anyway). Two sides, two constraints.

## Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ChunkError {
    /// Decode: the chunk's framing is unreadable (bad magic, unknown
    /// format_version, block before heading, unexpected item type, a
    /// crc-passing body that fails to decode, …).
    #[error("malformed chunk: {0}")]
    Malformed(&'static str),
    /// Encode: minicbor failed to serialize (never fires in practice — the
    /// `Vec` writer is `Infallible` and a `PersistedEnvelope` guarantees every
    /// field cap — but the type is honest about minicbor's fallible API).
    #[error("chunk encode failed: {0}")]
    Encode(#[from] minicbor::encode::Error<core::convert::Infallible>),
}
```

A dedicated box-layer error, not `ImportError<E, I>` — the box has no store error
`E` and no id type `I`. Two variants for two failure domains (CLAUDE rule 3):
`Malformed` is the decode-side framing rejection (mirrors the reserved
`ImportError::Malformed` slot but at the box layer); `Encode` preserves the
minicbor source via `#[from]` so encode can use `?` without discarding the error.
`Malformed`'s `&'static str` carries a debug hint ("bad magic", "unknown format
version", "block before section heading", "unexpected item type", "body decode
failed", …). `ChunkError` does **not** derive `PartialEq` (minicbor's error type
is not `PartialEq`); decode tests match on `ChunkError::Malformed(_)` via
`matches!` and assert the hint string separately.

## Feature wiring

`crates/nexus-store/Cargo.toml`:

```toml
cbor = ["import", "export", "dep:minicbor", "dep:crc32c"]
```

- `minicbor` locked over ciborium: no_std / no-serde, so the box stays decoupled
  from serde (rkyv/bytemuck users export without pulling serde). Pin via
  `cargo add` (2.2.2 verified for the layout). `#[cbor(with =
  "minicbor::bytes")]` on **every** bstr field (magic, origin, stream_id,
  metadata, payload, block body) — without it minicbor encodes `&[u8]` as a
  CBOR array-of-integers (wrong major type, ~4× bloat). `event_type` stays plain
  `&str` (tstr).
- `crc32c` crate for the Castagnoli per-block checksum. Pin via `cargo add`.

Add `cbor` to the `nexus-store` self dev-dependency feature list so the box
tests run under `nix flake check` (the gate runs default features only).

`lib.rs`: `#[cfg(feature = "cbor")] pub mod cbor;` + re-export
`encode_header`, `encode_section_heading`, `encode_block`, `decode_chunk`,
`decode_header`, `ChunkHeader`, `ChunkError`.

## Test plan (TDD, 4 cross-cutting categories first, then golden + VOPR)

1. **Sequence/protocol** — encode a multi-stream chunk (header + 2 headings +
   their blocks) → `decode_chunk` → exact `Vec<StreamSection>` back (origins +
   per-block version / event_type / payload / metadata / schema). Full pipeline:
   Card-1 export → box encode → `decode_chunk` → Card-2 `import` into a fresh
   `InMemoryStore` → streams byte-equal modulo `global_seq`.
2. **Lifecycle (incremental/append)** — a chunk is valid at every block-boundary
   prefix; truncating after N complete blocks decodes to those N (valid prefix);
   a torn final block is dropped, earlier blocks survive; header-only chunk →
   empty `Vec`.
3. **Defensive boundary** — flip a byte inside a block body → crc mismatch →
   that block decodes to `ImportBlock::Corrupt` (and the importer halts that
   stream); bad magic / unknown `format_version` → `Malformed`; block-before-
   heading → `Malformed`; oversize event_type/metadata or version 0 with a
   matching crc → `Malformed`; unexpected major type → `Malformed`; arbitrary
   garbage bytes → never panics, returns `Malformed` or a valid-prefix `Vec`.
4. **Linearizability/isolation** — encode is pure/stateless free functions and
   decode is pure, so there is no shared mutable state to race; the
   concurrency story is covered by Card 2's import tests. We assert the
   functions are `Send`-friendly (`Bytes` / owned outputs) and that
   encode→decode is deterministic across threads.
5. **insta golden bytes** — snapshot the encoded header and a known block's hex
   so any accidental format drift must consciously update the snapshot.
6. **VOPR / round-trip property test (proptest)** — for arbitrary valid
   `(origin, [streams of [events]])`, `decode_chunk(encode(x)) == x` modulo
   `global_seq`; and crc rejects any single-byte mutation of a body (decodes to
   `Corrupt`, never silently wrong). Boundary-inclusive strategies: empty
   payload, max event_type, empty/Some metadata, version/schema at 1 and
   near-max.
7. **Forward-compat** — a body carrying an unknown extra integer key still
   decodes to `Event` (decoders skip unknown keys).

Test modules carry a scoped
`#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "...")]`.

## Out of scope

- Live/continuous export loop, per-device file layout, iCloud transport, CRDT
  merge — all Agency (agency#147).
- A lazy streaming decoder (materialized `decode_chunk` suffices for import).
- Any change to the wire frame, `PersistedEnvelope`, or Cards 0–2.
- Signing / authenticity (consumer's CESR/KERI layer) and content-addressing.
