# Frame-format-version byte — design

**Issue:** [#205](https://github.com/devrandom-labs/nexus/issues/205)
**Date:** 2026-06-25
**Status:** approved, pre-implementation
**Tier:** 0 — irreversible once real on-disk data exists (1.0 freeze blocker)
**Scope:** `nexus-store::wire` only. Consumers (`nexus-fjall`, `codec`,
`testing`, benches) go through `encode_frame`/`decode_frame` and the shifted
public offset constants, so they need no logic change.

## Summary

The wire frame carries a per-event **schema** version (the event's own schema
evolution) but **no frame/wire-format version** (the byte layout itself). The
16-byte payload alignment, field order, the `meta_len == u32::MAX` absent
sentinel, and "payload length is derived, not stored" are all baked in with no
escape hatch. Once a `FjallStore` on disk holds rows in this exact layout, the
decoder cannot tell *which* layout a row uses, so the frame can never evolve
(32-byte align for AVX, a CRC, a stored payload length for truncation
detection) without a full data migration across every deployed store.

This is the only pre-freeze window to make the on-disk format evolvable. The
fix: prepend a self-describing **format-version tag** the decoder reads *first*,
before committing to any field offsets, so a future layout can coexist with v1
rows.

## Design decisions

### Dedicated leading byte, not stolen bits (approach A)

The version is a **dedicated 1-byte field prepended at offset 0**, not high
bits stolen from an existing field. Stealing bits from `schema_version` would
couple two unrelated concerns (frame layout vs. per-event schema evolution)
into one field and shrink `SchemaVersion`'s domain — exactly the "jam unrelated
things into one field" anti-pattern CLAUDE.md rule 3 warns against. A dedicated
byte keeps format-version as its own concept at a cost of one byte per row.

A magic number (option C) was rejected: the issue asks only for evolvability,
per-row magic costs 4+ bytes on the hot path, and it is redundant when fjall
already owns a whole partition of known-format rows.

### Typed enum for the branch seam (approach A2)

The version is modeled as a typed enum, not a bare `const u8`, so the decode
path performs a real `match` — the explicit "branch on layout" seam the issue
describes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormatVersion {
    V1,
}
```

The enum is **exhaustive** (no `#[non_exhaustive]`, per CLAUDE.md rule 3), so
the next variant is a compile-error-until-handled, one-line add. This is a
deliberate, small amount of structure justified by the fact that the whole task
is "make the format evolvable" — the branch point must be explicit and typed,
not an inline integer comparison.

## Components

### `FrameFormatVersion` (new)

```rust
impl FrameFormatVersion {
    /// The version every freshly-encoded frame is stamped with.
    pub const CURRENT: Self = Self::V1;

    /// On-wire byte for this version.
    const fn to_u8(self) -> u8 { match self { Self::V1 => 1 } }

    /// Write the version byte (1 byte) at the start of the frame buffer.
    fn write_into(self, buf: &mut AVec<u8, ConstAlign<PAYLOAD_ALIGN>>);

    /// Read and validate the leading version byte.
    ///
    /// - empty buffer        -> `DecodeError::ValueTooShort { min: 1, .. }`
    /// - byte == 1           -> `Ok(V1)`
    /// - any other byte      -> `DecodeError::UnsupportedFrameVersion { version }`
    fn read_from(value: &[u8]) -> Result<Self, DecodeError>;
}
```

### Layout & constants

The version byte is prepended at offset 0; the four fixed event fields shift by
one:

```text
[u8  frame_format_version]                          offset 0    <- new
[u64 LE global_seq]                                 offset 1
[u32 LE schema_version]                             offset 9
[u16 LE event_type_len]                             offset 13
[u32 LE meta_len]                                   offset 15
[event_type bytes][metadata bytes if any]
[padding zero-bytes][payload bytes]
```

Constant changes:

| constant | before | after |
|---|---|---|
| `FRAME_PREFIX_SIZE` | — | `1` (new) |
| `VERSION_OFFSET` | — | `0` (new) |
| `GLOBAL_SEQ_OFFSET` | `0` | `1` |
| `SCHEMA_VERSION_OFFSET` | `8` | `9` |
| `EVENT_TYPE_LEN_OFFSET` | `12` | `13` |
| `META_LEN_OFFSET` | `14` | `15` |
| `HEADER_FIXED_SIZE` | `18` | `19` (version + 18 fixed event fields) |

Because the layout math (`pre_payload_len = HEADER_FIXED_SIZE + et_len +
meta_len`) and the decode body (`et_start = HEADER_FIXED_SIZE`) both key off the
single `HEADER_FIXED_SIZE` constant, the variable-field and padding logic is
**unchanged** — it just begins one byte later.

### Alignment is provably preserved

The backing buffer is `AVec<u8, ConstAlign<16>>`, so its base is 16-aligned.
Payload alignment is recomputed by `align_padding` from the *actual*
pre-payload offset, so shifting the header by one byte only changes the padding
count, never the final payload position. Worked example (no metadata,
`et_len = 2`): pre-payload `20 -> 21`, padding `12 -> 11`, payload still lands
at offset 32 (16-aligned). Header fields are read via `from_le_bytes` on
**copied** arrays — never a direct unaligned pointer cast — so moving
`global_seq` to offset 1 is harmless.

### Encode / decode flow

**`encode_frame`** (via `plan`/`execute`): `execute` writes
`FrameFormatVersion::CURRENT.write_into(buf)` first, then `header.write_into`,
body, padding, payload — a symmetric write seam.

**`decode_frame`**: the explicit branch seam —

```rust
pub fn decode_frame(value: &[u8]) -> Result<DecodedFrame, DecodeError> {
    let version = FrameFormatVersion::read_from(value)?;
    match version {
        FrameFormatVersion::V1 => decode_frame_v1(value),
    }
}
```

`decode_frame_v1` holds today's decode logic with the shifted offsets;
`FrameHeader::read_from` reads the 18 fixed event fields starting at
`FRAME_PREFIX_SIZE`.

### Error handling

One new variant, its own failure domain (rule 3), distinct from
`ValueTooShort` / `CorruptSchemaVersion`, `thiserror`-derived:

```rust
#[error("unsupported frame format version on wire: got {version}")]
UnsupportedFrameVersion { version: u8 },
```

## Testing (4 mandatory categories first)

1. **Sequence / round-trip:** extend the `valid_frame_inputs` proptests — byte 0
   of every encoded frame `== 1`; `decode_frame` recovers `V1`; build -> decode
   round-trips are unchanged.
2. **Defensive boundary:** new test feeding the real `decode_frame` every byte
   `0` and `2..=255` at offset 0 -> `UnsupportedFrameVersion { version }`;
   empty buffer -> `ValueTooShort { min: 1 }`.
3. **Lifecycle / corruption:** existing tamper tests gain a version-byte-tamper
   case (flip offset 0, expect `UnsupportedFrameVersion`).
4. **Linearizability:** N/A for the pure codec (covered at the fjall layer).

Plus: update the hand-built buffer tests (`decode_rejects_truncated_*`, the
`layout_concrete_*` examples, `FrameHeader` round-trip) for
`HEADER_FIXED_SIZE = 19` and the new padding/offsets; extend
`adversarial_decode_bytes` to vary the leading version byte.

## Non-goals

- No second format version is implemented — `V1` is the only variant. The seam
  is in place; v2's actual shape (align change, CRC, stored payload length) is
  deferred until a concrete need defines it.
- No magic number / whole-store format header — per-row version is sufficient
  for the freeze guarantee.
