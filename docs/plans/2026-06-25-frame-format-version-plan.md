# Frame-format-version byte — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepend a self-describing 1-byte frame-format-version tag to the `nexus-store::wire` on-disk frame so the decoder can branch on layout, making the format evolvable before the 1.0 freeze.

**Architecture:** A typed `FrameFormatVersion` enum (one variant `V1`) becomes the first field of `FrameHeader`, serialized at byte offset 0. `FrameHeader::write_into`/`read_from` stay a true inverse pair (they own the version byte, so the existing header round-trip tests keep working). `decode_frame` reads the header, then does an explicit typed `match header.format_version { V1 => decode_frame_v1(..) }` — the evolvability seam. All four fixed event fields shift one byte later; `HEADER_FIXED_SIZE` absorbs the shift (`18 → 19`), so the variable-field/padding/alignment math is otherwise untouched.

**Tech Stack:** Rust 2024, `aligned_vec::AVec<u8, ConstAlign<16>>`, `bytes::Bytes`, `thiserror`, `proptest`. Single file: `crates/nexus-store/src/wire.rs`.

---

## Refinement vs. the design doc (intentional, behavior-identical)

The design doc (`docs/plans/2026-06-25-frame-format-version-design.md`) sketched `FrameFormatVersion::read_from`/`write_into` doing the byte I/O. During planning, byte I/O was moved **into `FrameHeader`** instead, and the enum exposes `to_u8` / `from_u8(u8) -> Option<Self>`. Reason: `FrameHeader` already owns header serialization; keeping the version byte there preserves the write/read **symmetry** that the existing `frame_header_round_trip` test relies on, and avoids reading the version byte twice. Everything user-visible from the design is preserved: version byte at offset 0, typed enum, explicit `match` seam in `decode_frame`, `DecodeError::UnsupportedFrameVersion { version: u8 }`.

## Gate constraint (why tasks are coarse)

The pre-commit hook runs the full `nix flake check` (clippy `-D warnings` + nextest) on **every** commit. A commit with an unused private method (`to_u8`/`from_u8` before they are wired in) fails clippy's dead-code denial, and a commit with red tests fails nextest. Therefore the enum must be introduced **and wired in** in the same commit, and existing tests updated in that same commit. Task 1 is consequently one atomic green commit; its internal steps are still small and ordered.

---

## File Structure

- **Modify only:** `crates/nexus-store/src/wire.rs` — the new enum, the shifted constants, the `FrameHeader` field, the encode/decode threading, the doc updates, and all tests live here. No other file changes (every consumer — `testing.rs`, `nexus-fjall`, `codec.rs`, benches — goes through `encode_frame`/`decode_frame` and the named offset constants, which shift transparently).

---

## Task 1: Introduce `FrameFormatVersion` end-to-end

**Files:**
- Modify: `crates/nexus-store/src/wire.rs`

This task adds the type, shifts the layout, threads the version byte through encode/decode, updates the in-code docs, and fixes the existing tests broken by the shift. It ends in one green commit.

- [ ] **Step 1: Add the `UnsupportedFrameVersion` error variant**

In `crates/nexus-store/src/wire.rs`, add a variant to `enum DecodeError` (after `CorruptSchemaVersion`):

```rust
    /// The leading frame-format-version byte holds a value this build does
    /// not understand. Distinct from `ValueTooShort` (not enough bytes) and
    /// `CorruptSchemaVersion` (per-event schema) — its own failure domain.
    #[error("unsupported frame format version on wire: got {version}, this build supports 1")]
    UnsupportedFrameVersion { version: u8 },
```

- [ ] **Step 2: Add the `FrameFormatVersion` enum**

Insert just below the constants block (after `META_LEN_ABSENT`, before `align_padding`):

```rust
/// On-disk frame-format version — the layout of the bytes themselves,
/// distinct from the per-event `schema_version`. Read first by the decoder
/// so a future layout (different alignment, a CRC, a stored payload length)
/// can coexist with `V1` rows. Exhaustive on purpose: adding the next
/// variant is a compile-error-forcing one-liner here and in `decode_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormatVersion {
    /// The original layout: see the module-level diagram.
    V1,
}

impl FrameFormatVersion {
    /// The version every freshly-encoded frame is stamped with.
    pub const CURRENT: Self = Self::V1;

    /// On-wire byte for this version.
    #[inline]
    const fn to_u8(self) -> u8 {
        match self {
            Self::V1 => 1,
        }
    }

    /// Map an on-wire byte to a known version, or `None` if unrecognized.
    /// The caller turns `None` into `DecodeError::UnsupportedFrameVersion`,
    /// so this stays decoupled from the error type.
    #[inline]
    const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::V1),
            _ => None,
        }
    }
}
```

- [ ] **Step 3: Shift the layout constants**

Replace the offset/size constants block. New values:

```rust
/// Payload alignment in bytes. Wire-format invariant.
pub const PAYLOAD_ALIGN: usize = 16;

/// Fixed header size: `frame_format_version (1) + global_seq (8)
/// + schema_version (4) + et_len (2) + meta_len (4)`.
pub const HEADER_FIXED_SIZE: usize = 19;

/// Offset of the `frame_format_version` byte in the frame's header.
pub const VERSION_OFFSET: usize = 0;

/// Offset of the `global_seq` field in the frame's header.
pub const GLOBAL_SEQ_OFFSET: usize = 1;

/// Offset of the `schema_version` field in the frame's header.
pub const SCHEMA_VERSION_OFFSET: usize = 9;

/// Offset of the `event_type_len` field in the frame's header.
pub const EVENT_TYPE_LEN_OFFSET: usize = 13;

/// Offset of the `meta_len` field in the frame's header.
pub const META_LEN_OFFSET: usize = 15;

/// Sentinel `meta_len` value meaning "no metadata field present".
pub const META_LEN_ABSENT: u32 = u32::MAX;
```

- [ ] **Step 4: Add `format_version` to `FrameHeader` and update its methods**

Add the field (first) to the struct:

```rust
pub struct FrameHeader {
    pub format_version: FrameFormatVersion,
    pub global_seq: u64,
    pub schema_version: u32,
    event_type_len: u16,
    metadata_len: Option<u32>,
}
```

Change `from_validated_lengths` to take and store the version (add as first param; set it in the returned `Self`):

```rust
    fn from_validated_lengths(
        format_version: FrameFormatVersion,
        global_seq: u64,
        schema_version: u32,
        event_type_len: usize,
        metadata_len: Option<usize>,
    ) -> Self {
        // ... existing u16/u32 try_from bodies unchanged ...
        Self {
            format_version,
            global_seq,
            schema_version,
            event_type_len: event_type_len_u16,
            metadata_len: metadata_len_u32,
        }
    }
```

Change `write_into` to emit the version byte first (writes exactly 19 bytes now):

```rust
    /// Serialize this header into the start of `buf` (writes exactly 19 bytes:
    /// the version byte then the four fixed fields). Inverse of `read_from`.
    fn write_into(&self, buf: &mut AVec<u8, ConstAlign<PAYLOAD_ALIGN>>) {
        let meta_field = self.metadata_len.unwrap_or(META_LEN_ABSENT);
        buf.extend_from_slice(&[self.format_version.to_u8()]);
        buf.extend_from_slice(&self.global_seq.to_le_bytes());
        buf.extend_from_slice(&self.schema_version.to_le_bytes());
        buf.extend_from_slice(&self.event_type_len.to_le_bytes());
        buf.extend_from_slice(&meta_field.to_le_bytes());
    }
```

Change `read_from` to read+validate the version byte first (length check stays first so a too-short buffer reports `ValueTooShort`, not a spurious version error), then the four fields at their new offsets:

```rust
    /// Read the fixed header from the start of `value`.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::ValueTooShort`] if `value.len() < SIZE`.
    /// - [`DecodeError::UnsupportedFrameVersion`] if the leading version byte
    ///   is not a known [`FrameFormatVersion`].
    pub fn read_from(value: &[u8]) -> Result<Self, DecodeError> {
        if value.len() < Self::SIZE {
            return Err(DecodeError::ValueTooShort {
                min: Self::SIZE,
                actual: value.len(),
            });
        }
        let version_byte = value[VERSION_OFFSET];
        let format_version = FrameFormatVersion::from_u8(version_byte)
            .ok_or(DecodeError::UnsupportedFrameVersion { version: version_byte })?;
        let global_seq = u64::from_le_bytes([
            value[GLOBAL_SEQ_OFFSET],
            value[GLOBAL_SEQ_OFFSET + 1],
            value[GLOBAL_SEQ_OFFSET + 2],
            value[GLOBAL_SEQ_OFFSET + 3],
            value[GLOBAL_SEQ_OFFSET + 4],
            value[GLOBAL_SEQ_OFFSET + 5],
            value[GLOBAL_SEQ_OFFSET + 6],
            value[GLOBAL_SEQ_OFFSET + 7],
        ]);
        let schema_version = u32::from_le_bytes([
            value[SCHEMA_VERSION_OFFSET],
            value[SCHEMA_VERSION_OFFSET + 1],
            value[SCHEMA_VERSION_OFFSET + 2],
            value[SCHEMA_VERSION_OFFSET + 3],
        ]);
        let event_type_len = u16::from_le_bytes([
            value[EVENT_TYPE_LEN_OFFSET],
            value[EVENT_TYPE_LEN_OFFSET + 1],
        ]);
        let meta_field = u32::from_le_bytes([
            value[META_LEN_OFFSET],
            value[META_LEN_OFFSET + 1],
            value[META_LEN_OFFSET + 2],
            value[META_LEN_OFFSET + 3],
        ]);
        let metadata_len = if meta_field == META_LEN_ABSENT {
            None
        } else {
            Some(meta_field)
        };
        Ok(Self {
            format_version,
            global_seq,
            schema_version,
            event_type_len,
            metadata_len,
        })
    }
```

(`FrameHeader::SIZE` is already `pub const SIZE: usize = HEADER_FIXED_SIZE;` — it now reads 19 automatically.)

- [ ] **Step 5: Stamp `CURRENT` in `plan`**

In `fn plan`, pass the current version into the header constructor:

```rust
    let header = FrameHeader::from_validated_lengths(
        FrameFormatVersion::CURRENT,
        global_seq,
        schema_version.get(),
        event_type_bytes.len(),
        metadata_bytes.map(<[u8]>::len),
    );
```

(`execute` is unchanged: it already calls `plan.header.write_into(&mut buf)` first, which now writes the 19-byte header including the version byte. `FrameLayout::compute_from_validated_lengths` and the decode body already key off `HEADER_FIXED_SIZE`, so the variable-field/padding math shifts automatically.)

- [ ] **Step 6: Split `decode_frame` into a dispatch + `decode_frame_v1`**

Replace the body of `decode_frame` with the version dispatch, and move today's logic into a private `decode_frame_v1` that takes the already-parsed header:

```rust
pub fn decode_frame(value: &[u8]) -> Result<DecodedFrame, DecodeError> {
    let header = FrameHeader::read_from(value)?;
    match header.format_version {
        FrameFormatVersion::V1 => decode_frame_v1(value, header),
    }
}

/// Decode the body of a `V1` frame given its already-validated header.
fn decode_frame_v1(value: &[u8], header: FrameHeader) -> Result<DecodedFrame, DecodeError> {
    let schema_version = SchemaVersion::from_u32(header.schema_version)
        .map_err(|_| DecodeError::CorruptSchemaVersion)?;
    let et_len = usize::from(header.event_type_len);

    let et_start = HEADER_FIXED_SIZE;
    // ... the remainder of the current decode_frame body, verbatim,
    //     from `let et_end = et_start.checked_add(et_len) ...`
    //     down to the final `Ok(DecodedFrame { ... })`.
}
```

The body from `let et_end = ...` through the returned `Ok(DecodedFrame { ... })` is copied verbatim — it already uses `HEADER_FIXED_SIZE` for `et_start` and the named offsets, so no inner edits are needed.

- [ ] **Step 7: Update the in-code docs**

Update three doc spots so the source stays accurate:

1. Module-level layout diagram (top of file) — change both occurrences (module doc and the `encode_frame` doc) to:

```text
//! [u8 frame_format_version][u64 LE global_seq][u32 LE schema_version]
//! [u16 LE event_type_len][u32 LE meta_len][event_type bytes]
//! [metadata bytes if any][padding zero-bytes][payload bytes]
```

2. In the module-level "Implicit couplings" section, add a bullet:

```text
//! - **The leading byte is the frame-format version.** `decode_frame`
//!   reads it first and branches on layout; an unknown version is a
//!   typed `DecodeError::UnsupportedFrameVersion`, never a misparse.
```

3. The `HEADER_FIXED_SIZE` doc comment already updated in Step 3.

- [ ] **Step 8: Fix existing tests broken by the shift**

Apply these exact edits in `mod tests`:

**a.** `layout_concrete_no_metadata_example` — `compute_from_validated_lengths(2, None, 1)` now yields `pre_payload = 21`, `padding = 11`, `event_type = 19..21` (payload `32..33` and `total = 33` are unchanged):

```rust
        assert_eq!(layout.padding, 11);
        assert_eq!(layout.event_type, 19..21);
        assert_eq!(layout.metadata, None);
        assert_eq!(layout.payload, 32..33);
        assert_eq!(layout.total, 33);
```

**b.** `layout_concrete_with_metadata_example` — `compute_from_validated_lengths(2, Some(3), 4)` now yields `pre_payload = 24`, `padding = 8`, `event_type = 19..21`, `metadata = 21..24` (payload `32..36`, `total = 36` unchanged):

```rust
        assert_eq!(layout.event_type, 19..21);
        assert_eq!(layout.metadata, Some(21..24));
        assert_eq!(layout.padding, 8);
        assert_eq!(layout.payload, 32..36);
        assert_eq!(layout.total, 36);
```

**c.** `frame_header_write_into_writes_all_fields_at_correct_offsets` — add `format_version` to the literal, assert the version byte, keep the named-offset asserts (they auto-shift), and assert `buf.len() == 19`:

```rust
        let header = FrameHeader {
            format_version: FrameFormatVersion::V1,
            global_seq: 0x0102_0304_0506_0708,
            schema_version: 0x090A_0B0C,
            event_type_len: 0x0D0E,
            metadata_len: Some(0x0F10_1112),
        };
        let mut buf = fresh_buf();
        header.write_into(&mut buf);

        assert_eq!(buf.len(), FrameHeader::SIZE);
        assert_eq!(buf[VERSION_OFFSET], 1);
        assert_eq!(
            &buf[GLOBAL_SEQ_OFFSET..GLOBAL_SEQ_OFFSET + 8],
            &0x0102_0304_0506_0708u64.to_le_bytes(),
        );
        assert_eq!(
            &buf[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4],
            &0x090A_0B0Cu32.to_le_bytes(),
        );
        assert_eq!(
            &buf[EVENT_TYPE_LEN_OFFSET..EVENT_TYPE_LEN_OFFSET + 2],
            &0x0D0Eu16.to_le_bytes(),
        );
        assert_eq!(
            &buf[META_LEN_OFFSET..META_LEN_OFFSET + 4],
            &0x0F10_1112u32.to_le_bytes(),
        );
```

**d.** `frame_header_none_metadata_encodes_sentinel` and `frame_header_some_zero_metadata_distinct_from_none` — add `format_version: FrameFormatVersion::V1,` as the first field of each `FrameHeader { .. }` literal. No other change (they read `META_LEN_OFFSET`, which auto-shifts, and round-trip through `read_from`).

**e.** `frame_header_read_from_accepts_exactly_size` — an all-zero `SIZE` buffer now fails the version check, so set the version byte and assert it round-trips:

```rust
    #[test]
    fn frame_header_read_from_accepts_exactly_size() {
        let mut buf = vec![0u8; FrameHeader::SIZE];
        buf[VERSION_OFFSET] = 1;
        let header = FrameHeader::read_from(&buf).expect("accepts at SIZE");
        assert_eq!(header.format_version, FrameFormatVersion::V1);
        assert_eq!(header.global_seq, 0);
        assert_eq!(header.schema_version, 0);
        assert_eq!(header.event_type_len, 0);
        assert_eq!(header.metadata_len, Some(0));
    }
```

(`frame_header_read_from_rejects_buffer_below_size` needs **no** change: every length in `0..SIZE` still fails the length check first, and `FrameHeader::SIZE` now reads 19.)

**f.** `frame_header_round_trip` proptest — add `format_version: FrameFormatVersion::V1,` as the first field of the `FrameHeader { .. }` literal, and add one assertion after the read-back:

```rust
            prop_assert_eq!(read.format_version, original.format_version);
```

**g.** `decode_rejects_truncated_event_type` — set the version byte so the version check passes and the test reaches the event-type-truncation arm (buffer is `vec![0u8; HEADER_FIXED_SIZE]`, now 19; the named-offset writes auto-shift). Add after allocating `buf`:

```rust
        buf[VERSION_OFFSET] = 1;
```

**h.** `decode_rejects_truncated_metadata` — same one-line addition after allocating `buf`:

```rust
        buf[VERSION_OFFSET] = 1;
```

(`decode_rejects_truncated_value` needs no change: 18 zero bytes < 19 fails the length check first. `decode_rejects_corrupt_schema_version_zero`, `decode_frame_rejects_corrupt_schema_version_zero`, `header_fields_are_recoverable`, `meta_len_u32_max_is_absent_sentinel`, and all encode→decode round-trip proptests go through real `encode_frame`, which now writes a valid version byte, so they pass unchanged.)

- [ ] **Step 9: Commit (runs the gate)**

```bash
git add crates/nexus-store/src/wire.rs
git commit -m "feat(store): add frame-format-version byte to wire layout (#205)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

Expected: the pre-commit hook runs `nix flake check` and prints `Flake check passed.` (clippy clean, all `nexus-store` wire tests green).

---

## Task 2: Add dedicated version-byte tests (4 mandatory categories)

**Files:**
- Modify: `crates/nexus-store/src/wire.rs` (`mod tests`)

These add coverage specific to the new field. Each new test calls the real `encode_frame`/`decode_frame`/`FrameHeader::read_from`.

- [ ] **Step 1: Round-trip / sequence — version byte is stamped and recovered**

Add inside the existing `proptest! { .. }` block that uses `valid_frame_inputs()`:

```rust
        #[test]
        fn encoded_frame_carries_v1_version_byte(
            (global_seq, schema_version, event_type, metadata, payload) in valid_frame_inputs(),
        ) {
            let et_v = et(&event_type);
            let pl_v = pl(&payload);
            let md_v = metadata.as_deref().map(md);
            let frame = encode_frame(global_seq, schema_version, &et_v, &pl_v, md_v.as_ref())
                .expect("encode_frame succeeds on bounded inputs");
            // Sequence: the leading byte is the version tag, == 1.
            prop_assert_eq!(frame.value[VERSION_OFFSET], 1);
            // And the header reader recovers it as the typed V1.
            let header = FrameHeader::read_from(&frame.value)
                .expect("header reads back from a freshly built frame");
            prop_assert_eq!(header.format_version, FrameFormatVersion::V1);
        }
```

- [ ] **Step 2: Defensive boundary — every non-`1` version byte is rejected**

Add as a standalone `#[test]` (full-length frame so the length check passes and the version check is what fires):

```rust
    #[test]
    fn decode_rejects_every_unknown_version_byte() {
        // A valid V1 frame, then flip offset 0 to each non-1 byte.
        let frame = encode_frame(7, sv1(), &et("Evt"), &pl(b"payload"), Some(&md(b"m")))
            .expect("valid frame for tamper base");
        for bad in (0u8..=u8::MAX).filter(|b| *b != 1) {
            let mut bytes_vec = frame.value.to_vec();
            bytes_vec[VERSION_OFFSET] = bad;
            let tampered = Bytes::from(bytes_vec);
            match decode_frame(&tampered) {
                Err(DecodeError::UnsupportedFrameVersion { version }) => {
                    assert_eq!(version, bad);
                }
                other => panic!("version byte {bad} should be rejected, got {other:?}"),
            }
        }
    }
```

- [ ] **Step 3: Defensive boundary — empty buffer reports too-short, not a version error**

```rust
    #[test]
    fn decode_empty_buffer_is_too_short_not_version_error() {
        match decode_frame(&[]) {
            Err(DecodeError::ValueTooShort { min, actual }) => {
                assert_eq!(min, HEADER_FIXED_SIZE);
                assert_eq!(actual, 0);
            }
            other => panic!("empty buffer should be ValueTooShort, got {other:?}"),
        }
    }
```

- [ ] **Step 4: Lifecycle / corruption — a flipped version byte on a stored frame surfaces typed**

```rust
    #[test]
    fn corrupt_version_byte_surfaces_unsupported_not_panic() {
        // Simulate on-disk bit-rot of byte 0 of a persisted frame.
        let frame = encode_frame(1, sv1(), &et("X"), &pl(b"p"), None).expect("encode");
        let mut bytes_vec = frame.value.to_vec();
        bytes_vec[VERSION_OFFSET] = 0xFF;
        let tampered = Bytes::from(bytes_vec);
        let err = decode_frame(&tampered).expect_err("corrupt version rejected");
        assert!(matches!(
            err,
            DecodeError::UnsupportedFrameVersion { version: 0xFF }
        ));
    }
```

- [ ] **Step 5: Restore adversarial deep-path coverage with a version byte**

`adversarial_decode_bytes`'s `header_shaped` arm currently lays `global_seq` at offset 0, so after the shift its first byte is a random version that almost always short-circuits to `UnsupportedFrameVersion`, starving the metadata/payload arms. Prepend a version byte (usually valid, sometimes random) so the deep paths are exercised again. Replace the `header_shaped` definition with:

```rust
        // Header-shaped: leading version byte (mostly the valid 1, sometimes
        // random to exercise the version-reject path), then small et_len /
        // meta_len, random body. Drives the deeper code paths that raw
        // random rarely reaches.
        let header_shaped = (
            prop_oneof![10 => Just(1u8), 1 => any::<u8>()],
            any::<u64>(),
            any::<u32>(),
            0u16..=64,
            prop_oneof![Just(META_LEN_ABSENT), 0u32..=64],
            prop::collection::vec(any::<u8>(), 0..=512),
        )
            .prop_map(|(version, gs, sv, et_len, meta_len, body)| {
                let mut buf = Vec::with_capacity(HEADER_FIXED_SIZE + body.len());
                buf.extend_from_slice(&[version]);
                buf.extend_from_slice(&gs.to_le_bytes());
                buf.extend_from_slice(&sv.to_le_bytes());
                buf.extend_from_slice(&et_len.to_le_bytes());
                buf.extend_from_slice(&meta_len.to_le_bytes());
                buf.extend_from_slice(&body);
                buf
            });
```

(The `decode_never_panics` and `decode_offsets_in_bounds_on_success` proptests that consume this strategy need no change — they already tolerate any `Result`.)

- [ ] **Step 6: Commit (runs the gate)**

```bash
git add crates/nexus-store/src/wire.rs
git commit -m "test(store): cover frame-format-version byte across the 4 categories (#205)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

Expected: pre-commit hook prints `Flake check passed.`

---

## Self-review

**Spec coverage:**
- Dedicated leading byte (approach A) → Task 1 Steps 3–4 (constants + `FrameHeader` field at `VERSION_OFFSET = 0`). ✓
- Typed enum branch seam (approach A2) → Task 1 Step 2 (enum) + Step 6 (`match` in `decode_frame`). ✓
- `HEADER_FIXED_SIZE 18 → 19`, offsets shift → Step 3. ✓
- Alignment preserved → no change to `align_padding`/`AVec<ConstAlign<16>>`; verified by the unchanged `payload_pointer_is_16_aligned` / `execute_invariants` proptests + updated concrete examples (8a/8b). ✓
- `DecodeError::UnsupportedFrameVersion { version: u8 }` (own domain, `thiserror`) → Step 1. ✓
- Testing: round-trip (T2 S1), defensive boundary (T2 S2/S3), corruption/lifecycle (T2 S4), adversarial (T2 S5); existing hand-built buffers updated (T1 S8). ✓
- Non-goal "no v2 implemented" → only `V1` variant; seam left as a one-arm `match`. ✓

**Placeholder scan:** the only "verbatim copy" reference is Task 1 Step 6's reuse of the existing decode body — its source is the current `decode_frame` in the same file, fully present, not a placeholder. No TBD/TODO.

**Type consistency:** `FrameFormatVersion` (`V1`, `CURRENT`, `to_u8`, `from_u8`), `FrameHeader.format_version`, `HEADER_FIXED_SIZE`, `VERSION_OFFSET`, `DecodeError::UnsupportedFrameVersion { version }`, `decode_frame_v1(value, header)` are referenced consistently across all steps. (`FRAME_PREFIX_SIZE` from the design doc was dropped — unused once `HEADER_FIXED_SIZE`/`VERSION_OFFSET` describe the layout; rule 4.)
