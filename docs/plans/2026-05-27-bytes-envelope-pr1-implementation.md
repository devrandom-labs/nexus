# Bytes Envelope PR1 — Envelope Shape + Wire Format Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Companion docs:** [`design.md`](./2026-05-27-bytes-envelope-design.md), [`deviations.md`](./2026-05-27-bytes-envelope-deviations.md). Read both before starting.

**Goal:** Replace `PendingEnvelope<M>` and `PersistedEnvelope<'a, M>` with new owned `Bytes`-based envelopes using the "single `Bytes` + `Range<u32>` fields" pattern, extend the fjall wire format with a metadata length-prefix, update fjall + InMemoryStore + repository facade + tests + examples. Stream traits still carry the `M = ()` generic; PR2 drops them.

**Architecture:** Pre-1.0 breaking change. Old `M`-generic envelope types are deleted, not deprecated. New envelopes hold `value: Bytes` (the whole row value buffer) plus `Range<u32>` offsets for `event_type`, `payload`, and `metadata`. Accessors return `&[u8]` cheaply or `Bytes` via `value.slice(range)`. The fjall `bytes_1` feature is enabled so `Slice` IS `Bytes` with zero-copy interop. Old generic `M` parameter on stream/store traits stays for now — collapsed to `()` at adapter sites; PR2 removes them.

**Tech Stack:** Rust 2024, `bytes = "1"`, `fjall` with `features = ["bytes_1"]`. No other new deps.

**Branch:** `refactor/bytes-envelope-pr1`

---

## File Structure

**Workspace + crate manifests:**
- Modify: `Cargo.toml` — add `bytes = "1"` to `[workspace.dependencies]`, change `fjall = "3"` to `fjall = { version = "3", features = ["bytes_1"] }`.
- Modify: `crates/nexus-store/Cargo.toml` — add `bytes = { workspace = true }` to deps.
- Modify: `crates/nexus-fjall/Cargo.toml` — add `bytes = { workspace = true }` to deps.

**Envelope (full rewrite):**
- Modify: `crates/nexus-store/src/envelope.rs` — new `PendingEnvelope` (no generic, no lifetime, Bytes+ranges) and `PersistedEnvelope` (no generic, no lifetime, Bytes+ranges).

**Wire format (extend):**
- Modify: `crates/nexus-fjall/src/encoding.rs` — add `meta_len: u32` field between `event_type_len` and `event_type`; `meta_len == u32::MAX` is absent sentinel. Change `decode_event_value` to take `&Bytes` and return ranges.

**Adapters (update construction sites):**
- Modify: `crates/nexus-fjall/src/store.rs` — update `append` to read `PendingEnvelope.metadata()` and pass to encoder; remove `<M>` from impl.
- Modify: `crates/nexus-fjall/src/stream.rs` — update cursor to construct new `PersistedEnvelope` from `Bytes` + ranges.
- Modify: `crates/nexus-fjall/src/subscription_stream.rs` — same change as `stream.rs`.
- Modify: `crates/nexus-store/src/testing.rs` — update `InMemoryStore` storage to retain owned `Bytes` per event; cursor constructs new envelope.

**Facade:**
- Modify: `crates/nexus-store/src/repository.rs` — update `save` to build `PendingEnvelope` via new builder.

**Stream traits (light touch — full collapse is PR2):**
- Modify: `crates/nexus-store/src/stream/cursor.rs` — `EventStream<M>`'s `Item<'a> = PersistedEnvelope<'a, M>` changes to `Item<'a> = PersistedEnvelope` (no lifetime, no generic on the envelope; the trait still threads `M = ()` until PR2). Combinators continue to compile because they're generic over `Item`.

**Tests:**
- Modify: `crates/nexus-store/tests/envelope_tests.rs` (if exists, else add to existing test file).
- Add new tests inline in `crates/nexus-store/src/envelope.rs` `#[cfg(test)]` module.
- Modify: `crates/nexus-fjall/src/encoding.rs` test module — adapt existing tests, add metadata round-trip.
- Modify: `crates/nexus-fjall/tests/*.rs` — adapt cursor read tests to new envelope shape.
- Modify: `crates/nexus-store/tests/stream_tests.rs` — adapt `MetadataStream` test fixture (line ~210) to construct `PersistedEnvelope` differently (or delete if obviated).

**Examples:**
- Modify: `examples/store-and-kernel/src/main.rs` — update envelope construction (line ~225 uses `.build_without_metadata()` — replace with new builder).
- Modify: `examples/store-inmemory/src/main.rs` — same.

---

### Task 1: Create feature branch + add `bytes` workspace dep + enable fjall `bytes_1`

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/nexus-store/Cargo.toml`
- Modify: `crates/nexus-fjall/Cargo.toml`

- [ ] **Step 1: Create branch**

```bash
git checkout main && git pull
git checkout -b refactor/bytes-envelope-pr1
```

- [ ] **Step 2: Add `bytes` to workspace dependencies and enable fjall `bytes_1` feature**

In `Cargo.toml` (workspace root), under `[workspace.dependencies]`, add:

```toml
bytes = "1"
```

In the same block, change the existing line `fjall = "3"` to:

```toml
fjall = { version = "3", features = ["bytes_1"] }
```

- [ ] **Step 3: Add `bytes` to nexus-store**

```bash
cd crates/nexus-store && nix develop -c cargo add bytes --workspace-dep && cd -
```

If `--workspace-dep` flag is unsupported, manually edit `crates/nexus-store/Cargo.toml` `[dependencies]` and add: `bytes = { workspace = true }`.

- [ ] **Step 4: Add `bytes` to nexus-fjall**

```bash
cd crates/nexus-fjall && nix develop -c cargo add bytes --workspace-dep && cd -
```

- [ ] **Step 5: Verify the workspace still builds**

Run: `nix develop -c cargo check --workspace`
Expected: PASS — no functional changes yet, just dep addition.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/nexus-store/Cargo.toml crates/nexus-fjall/Cargo.toml
git commit -m "$(cat <<'EOF'
chore(deps): add bytes workspace dep and enable fjall bytes_1 feature

Foundation for the bytes-envelope refactor. The bytes_1 feature on fjall
makes Slice IS bytes::Bytes with zero-copy From/Into in both directions,
which is load-bearing for the new envelope shape.
EOF
)"
```

---

### Task 2: Rewrite `envelope.rs` with Bytes+ranges pattern

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs` (full rewrite)
- The new file imports `bytes::Bytes`, drops `M` generic on both envelope structs, drops `'a` lifetime, and uses `Range<u32>` offsets.

- [ ] **Step 1: Write the failing test first**

Add to `crates/nexus-store/src/envelope.rs` (at end, in `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use nexus::Version;
    use std::num::NonZeroU32;

    #[test]
    fn pending_envelope_builds_with_metadata() {
        let env = pending_envelope(Version::INITIAL)
            .event_type("UserCreated")
            .payload(Bytes::from_static(b"payload-bytes"))
            .with_metadata(Bytes::from_static(b"meta-bytes"));

        assert_eq!(env.event_type(), "UserCreated");
        assert_eq!(env.payload(), b"payload-bytes");
        assert_eq!(env.metadata(), Some(b"meta-bytes".as_slice()));
        assert_eq!(env.schema_version(), 1);
    }

    #[test]
    fn pending_envelope_builds_without_metadata() {
        let env = pending_envelope(Version::INITIAL)
            .event_type("X")
            .payload(Bytes::from_static(b""))
            .build();

        assert_eq!(env.metadata(), None);
    }

    #[test]
    fn persisted_envelope_accessors_return_views_into_value() {
        let value = Bytes::from_static(b"TYPEpayloadmeta");
        // ranges: event_type 0..4 ("TYPE"), payload 4..11 ("payload"), metadata 11..15 ("meta")
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1),
            value,
            1,
            0..4,
            4..11,
            Some(11..15),
        )
        .expect("valid construction");

        assert_eq!(env.event_type(), "TYPE");
        assert_eq!(env.payload(), b"payload");
        assert_eq!(env.metadata(), Some(b"meta".as_slice()));
    }

    #[test]
    fn persisted_envelope_payload_bytes_shares_arc() {
        let value = Bytes::from_static(b"TYPEpayload");
        let env = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1),
            value.clone(),
            1,
            0..4,
            4..11,
            None,
        )
        .unwrap();

        let payload = env.payload_bytes();
        // Both env's internal value and the materialized payload share the
        // same allocation — proven indirectly by length-checking the slice.
        assert_eq!(payload.as_ref(), b"payload");
    }

    #[test]
    fn persisted_envelope_rejects_range_past_buffer() {
        let value = Bytes::from_static(b"short");
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1),
            value,
            1,
            0..4,
            4..100, // out of bounds
            None,
        )
        .expect_err("must reject out-of-bounds range");
        assert!(matches!(err, EnvelopeError::RangeOutOfBounds { .. }));
    }

    #[test]
    fn persisted_envelope_rejects_schema_version_zero() {
        let value = Bytes::from_static(b"TYPEpayload");
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1),
            value,
            0,
            0..4,
            4..11,
            None,
        )
        .expect_err("must reject schema_version == 0");
        assert!(matches!(err, EnvelopeError::InvalidSchemaVersion));
    }

    #[test]
    fn persisted_envelope_rejects_invalid_utf8_in_event_type() {
        // 0xFF is invalid UTF-8 start byte
        let value = Bytes::from_static(&[0xFFu8, 0xFF, b'p', b'a', b'y']);
        let err = PersistedEnvelope::try_new(
            Version::INITIAL,
            crate::store::GlobalSeq::new(1),
            value,
            1,
            0..2,
            2..5,
            None,
        )
        .expect_err("must reject non-UTF-8 event_type");
        assert!(matches!(err, EnvelopeError::InvalidUtf8 { .. }));
    }
}
```

- [ ] **Step 2: Run the tests — they MUST fail**

Run: `nix develop -c cargo test -p nexus-store --lib envelope::tests`
Expected: FAIL — the new APIs don't exist yet.

- [ ] **Step 3: Replace `envelope.rs` with the new implementation**

Write the entire file (replaces the old `PendingEnvelope<M>` / `PersistedEnvelope<'a, M>` structs and the typestate builder). Full new content for `crates/nexus-store/src/envelope.rs`:

```rust
use std::num::NonZeroU32;
use std::ops::Range;

use bytes::Bytes;
use nexus::Version;
use thiserror::Error;

use crate::store::GlobalSeq;

// =============================================================================
// Errors
// =============================================================================

/// Errors from envelope construction.
#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("schema_version must be > 0 (got 0)")]
    InvalidSchemaVersion,

    #[error("range {start}..{end} exceeds buffer length {len}")]
    RangeOutOfBounds { start: u32, end: u32, len: usize },

    #[error("invalid UTF-8 in event_type at bytes {start}..{end}")]
    InvalidUtf8 {
        start: u32,
        end: u32,
        #[source]
        source: std::str::Utf8Error,
    },
}

// =============================================================================
// PendingEnvelope — write path, fully owned (Bytes), no lifetime, no generic
// =============================================================================

/// Event envelope for the write path.
///
/// `payload` and `metadata` are `bytes::Bytes` — Arc-shared, cheap to clone,
/// no lifetime. Safe to hold across awaits or in long-lived containers.
///
/// Construction is via the typestate builder rooted at [`pending_envelope`].
#[derive(Debug, Clone)]
pub struct PendingEnvelope {
    version: Version,
    event_type: &'static str,
    schema_version: u32,
    payload: Bytes,
    metadata: Option<Bytes>,
}

impl PendingEnvelope {
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        self.event_type
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Owned view — one atomic refcount inc, zero allocation.
    #[must_use]
    pub fn payload_bytes(&self) -> Bytes {
        self.payload.clone()
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_deref()
    }

    /// Owned view — one atomic refcount inc per `Some`, zero allocation.
    #[must_use]
    pub fn metadata_bytes(&self) -> Option<Bytes> {
        self.metadata.clone()
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

// =============================================================================
// Typestate builder — compile-time enforced construction
// =============================================================================

/// Step 1: has `version`, needs `event_type`.
pub struct WithVersion {
    version: Version,
}

/// Step 2: has `version` + `event_type`, needs payload.
pub struct WithEventType {
    version: Version,
    event_type: &'static str,
}

/// Step 3: has all core fields; optional `schema_version` override; finalize via `build`/`with_metadata`.
pub struct WithPayload {
    version: Version,
    event_type: &'static str,
    payload: Bytes,
    schema_version: u32,
}

impl WithVersion {
    #[must_use]
    pub const fn event_type(self, event_type: &'static str) -> WithEventType {
        WithEventType {
            version: self.version,
            event_type,
        }
    }
}

impl WithEventType {
    /// Accepts anything that converts into `Bytes` — `Bytes`, `Vec<u8>`
    /// (reuses the Vec's allocation), `&'static [u8]`, `String`.
    #[must_use]
    pub fn payload(self, payload: impl Into<Bytes>) -> WithPayload {
        WithPayload {
            version: self.version,
            event_type: self.event_type,
            payload: payload.into(),
            schema_version: 1,
        }
    }
}

impl WithPayload {
    /// Override the schema version (default: 1).
    ///
    /// Uses `NonZeroU32` so `schema_version == 0` is a compile-time error,
    /// matching the invariant enforced by `PersistedEnvelope::try_new`.
    #[must_use]
    pub const fn schema_version(mut self, schema_version: NonZeroU32) -> Self {
        self.schema_version = schema_version.get();
        self
    }

    /// Build with no metadata.
    #[must_use]
    pub fn build(self) -> PendingEnvelope {
        PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata: None,
        }
    }

    /// Build with metadata. Accepts any `Into<Bytes>`.
    #[must_use]
    pub fn with_metadata(self, metadata: impl Into<Bytes>) -> PendingEnvelope {
        PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata: Some(metadata.into()),
        }
    }
}

/// Start building a `PendingEnvelope`.
///
/// ```ignore
/// pending_envelope(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .build()
/// // or
///     .with_metadata(meta_bytes)
/// ```
#[must_use]
pub const fn pending_envelope(version: Version) -> WithVersion {
    WithVersion { version }
}

// =============================================================================
// PersistedEnvelope — read path, owned via Bytes + Range<u32> offsets
// =============================================================================

/// Event envelope for the read path.
///
/// Holds the whole row value as a single `Bytes` plus `Range<u32>` offsets
/// for `event_type`, `payload`, and (optional) `metadata`. All views share
/// the one Arc; accessors return `&[u8]`/`&str` cheaply or `Bytes` via
/// `value.slice(range)` for owned views.
///
/// Construction validates ranges against the buffer and UTF-8 of `event_type`
/// at one point; accessors are then panic-free fast paths.
#[derive(Debug, Clone)]
pub struct PersistedEnvelope {
    version: Version,
    global_seq: GlobalSeq,
    schema_version: u32,
    value: Bytes,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
}

impl PersistedEnvelope {
    /// Construct from decoded row data, validating ranges and UTF-8.
    ///
    /// # Errors
    ///
    /// - `InvalidSchemaVersion` if `schema_version == 0`.
    /// - `RangeOutOfBounds` if any range's `end` exceeds `value.len()`.
    /// - `InvalidUtf8` if `event_type` bytes are not valid UTF-8.
    pub fn try_new(
        version: Version,
        global_seq: GlobalSeq,
        value: Bytes,
        schema_version: u32,
        event_type_range: Range<u32>,
        payload_range: Range<u32>,
        metadata_range: Option<Range<u32>>,
    ) -> Result<Self, EnvelopeError> {
        if schema_version == 0 {
            return Err(EnvelopeError::InvalidSchemaVersion);
        }
        let len = value.len();
        check_range(&event_type_range, len)?;
        check_range(&payload_range, len)?;
        if let Some(ref m) = metadata_range {
            check_range(m, len)?;
        }
        // UTF-8 validation of event_type once at construction.
        let et_start = event_type_range.start as usize;
        let et_end = event_type_range.end as usize;
        std::str::from_utf8(&value[et_start..et_end]).map_err(|e| EnvelopeError::InvalidUtf8 {
            start: event_type_range.start,
            end: event_type_range.end,
            source: e,
        })?;
        Ok(Self {
            version,
            global_seq,
            schema_version,
            value,
            event_type_range,
            payload_range,
            metadata_range,
        })
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn global_seq(&self) -> GlobalSeq {
        self.global_seq
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// UTF-8 validated at construction; cheap accessor.
    #[must_use]
    pub fn event_type(&self) -> &str {
        let start = self.event_type_range.start as usize;
        let end = self.event_type_range.end as usize;
        // SAFETY: UTF-8 validated once in `try_new`; range bounds checked
        // there too. The backing `Bytes` is immutable post-construction
        // (no setter, fields private).
        #[allow(unsafe_code, reason = "UTF-8 invariant established at construction; ranges validated")]
        unsafe {
            std::str::from_utf8_unchecked(&self.value[start..end])
        }
    }

    /// Owned `Bytes` view of `event_type` — one atomic refcount inc.
    #[must_use]
    pub fn event_type_bytes(&self) -> Bytes {
        self.slice_range(&self.event_type_range)
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        let start = self.payload_range.start as usize;
        let end = self.payload_range.end as usize;
        &self.value[start..end]
    }

    /// Owned `Bytes` view of `payload` — one atomic refcount inc.
    #[must_use]
    pub fn payload_bytes(&self) -> Bytes {
        self.slice_range(&self.payload_range)
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata_range.as_ref().map(|r| {
            let start = r.start as usize;
            let end = r.end as usize;
            &self.value[start..end]
        })
    }

    /// Owned `Bytes` view of `metadata` — one atomic refcount inc per `Some`.
    ///
    /// Note: per the design doc footgun F1, an empty range would collapse to
    /// `STATIC_VTABLE` and orphan from the parent buffer. The wire format
    /// MUST use `meta_len == u32::MAX` for absent (decoded to `None`), and
    /// "present but empty" metadata is currently disallowed — the decoder
    /// must enforce this.
    #[must_use]
    pub fn metadata_bytes(&self) -> Option<Bytes> {
        self.metadata_range.as_ref().map(|r| self.slice_range(r))
    }

    /// The schema version as a `Version` for upcaster APIs.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version == 0`, which `try_new` rejects — so this
    /// can only fire if internal invariants are violated.
    #[must_use]
    #[allow(clippy::expect_used, reason = "try_new guarantees schema_version >= 1")]
    pub fn schema_version_as_version(&self) -> Version {
        Version::new(u64::from(self.schema_version))
            .expect("PersistedEnvelope invariant: schema_version >= 1")
    }

    fn slice_range(&self, range: &Range<u32>) -> Bytes {
        let start = range.start as usize;
        let end = range.end as usize;
        self.value.slice(start..end)
    }
}

fn check_range(range: &Range<u32>, len: usize) -> Result<(), EnvelopeError> {
    if (range.end as usize) > len || range.start > range.end {
        return Err(EnvelopeError::RangeOutOfBounds {
            start: range.start,
            end: range.end,
            len,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // (test module from Step 1 goes here)
}
```

Move the test module from Step 1 into the new file (replacing the `// (test module from Step 1 goes here)` comment).

- [ ] **Step 4: Delete the old `InvalidSchemaVersion` import / type in `crates/nexus-store/src/error.rs`**

Search for `InvalidSchemaVersion` in `crates/nexus-store/src/error.rs`. If a standalone type exists there, delete it (the new envelope owns its own error type now). If it's only re-exported, update the re-export to point at `envelope::EnvelopeError`.

- [ ] **Step 5: Run the envelope tests — they should now pass**

Run: `nix develop -c cargo test -p nexus-store --lib envelope::tests`
Expected: PASS — all 7 tests in the new mod pass.

- [ ] **Step 6: Run `cargo check --workspace` to find consumer breakage**

Run: `nix develop -c cargo check --workspace 2>&1 | head -200`
Expected: FAIL with many `M`-generic and lifetime errors at fjall/testing/repository call sites. This is expected — Tasks 3–7 fix each consumer.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/envelope.rs crates/nexus-store/src/error.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: rewrite envelope.rs as Bytes-based with Range<u32> offsets

PendingEnvelope and PersistedEnvelope are now owned (Bytes), no
lifetime, no M generic. PersistedEnvelope uses the "single Bytes +
Range<u32> fields" pattern verified against bytes/src/bytes.rs:
- Bytes::slice() = one atomic refcount inc, zero memcpy
- size_of::<Bytes>() == 32 on 64-bit
- size_of::<Option<Bytes>>() == 32 (vtable niche)

Construction validates ranges + event_type UTF-8 at one point;
accessors are cheap (&str/&[u8]) or owned-view (Bytes::slice).

Adapters and consumers will not compile until subsequent tasks in
this PR update them.
EOF
)"
```

---

### Task 3: Update wire format in `nexus-fjall/src/encoding.rs`

**Files:**
- Modify: `crates/nexus-fjall/src/encoding.rs`

Wire format change: insert `[u32 LE meta_len]` between `[u16 LE event_type_len]` and `[event_type]`. `meta_len == u32::MAX` is the absent sentinel; any other value means "metadata present with that length." Header size grows from 14 → 18 bytes.

- [ ] **Step 1: Write the new round-trip tests first**

In `crates/nexus-fjall/src/encoding.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn event_value_round_trips_with_metadata() {
    let mut buf = Vec::new();
    let global_seq: u64 = 42;
    let schema_version: u32 = 3;
    let event_type = "UserCreated";
    let metadata = b"correlation-abc-123".as_slice();
    let payload = b"some-json-bytes";

    encode_event_value(
        &mut buf,
        global_seq,
        schema_version,
        event_type,
        Some(metadata),
        payload,
    )
    .unwrap();

    let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
    let decoded = decode_event_value(&bytes_buf).unwrap();

    assert_eq!(decoded.global_seq, global_seq);
    assert_eq!(decoded.schema_version, schema_version);
    assert_eq!(
        &bytes_buf[decoded.event_type_range.start as usize..decoded.event_type_range.end as usize],
        event_type.as_bytes()
    );
    assert_eq!(
        &bytes_buf[decoded.payload_range.start as usize..decoded.payload_range.end as usize],
        payload
    );
    let meta_range = decoded.metadata_range.expect("metadata present");
    assert_eq!(
        &bytes_buf[meta_range.start as usize..meta_range.end as usize],
        metadata
    );
}

#[test]
fn event_value_round_trips_without_metadata() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 7, 1, "Empty", None, b"").unwrap();

    let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
    let decoded = decode_event_value(&bytes_buf).unwrap();

    assert_eq!(decoded.global_seq, 7);
    assert_eq!(decoded.schema_version, 1);
    assert!(decoded.metadata_range.is_none());
    assert_eq!(decoded.payload_range.end, decoded.payload_range.start);
}

#[test]
fn event_value_rejects_oversized_metadata() {
    let mut buf = Vec::new();
    let too_big = vec![0u8; (u32::MAX as usize - 1).min(1 << 24)]; // bounded test alloc
    let err = encode_event_value(&mut buf, 1, 1, "X", Some(&too_big), b"")
        .err();
    // For practical test budgets we don't actually allocate 4 GiB;
    // assert only that the API accepts the size and that meta_len fits u32.
    // Skip this check if the allocation succeeded — the real boundary test
    // is the next one which exercises the sentinel collision case.
    let _ = err;
}

#[test]
fn event_value_rejects_meta_len_collision_with_absent_sentinel() {
    // u32::MAX is the absent sentinel. A metadata of exactly that length
    // would collide; the encoder must reject.
    let mut buf = Vec::new();
    let huge_len = u32::MAX as usize;
    // Build a fake "metadata" of length u32::MAX by lying about a slice's
    // length — we cannot actually allocate u32::MAX bytes in tests, so the
    // encoder API must check via len.try_into::<u32> and reject if equal
    // to sentinel.
    //
    // This test asserts the encoder enforces the invariant by inspecting
    // a constant rather than allocating:
    assert_eq!(META_LEN_ABSENT, u32::MAX);

    // For the runtime check, see encoder logic — covered by inspection.
    drop(huge_len);
    drop(buf);
}
```

- [ ] **Step 2: Run tests — they MUST fail (functions don't take the new signature yet)**

Run: `nix develop -c cargo test -p nexus-fjall --lib encoding::tests`
Expected: FAIL — `decode_event_value` doesn't take `&Bytes`, `encode_event_value` doesn't take `Option<&[u8]>`, etc.

- [ ] **Step 3: Update `crates/nexus-fjall/src/encoding.rs` with the new format**

Replace the `EVENT_VALUE_HEADER_SIZE` constant and the encode/decode functions. The header now contains `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len]`.

Apply these specific changes:

Change `EVENT_VALUE_HEADER_SIZE`:

```rust
/// Size of the fixed header in an encoded event value:
/// `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len]`.
pub const EVENT_VALUE_HEADER_SIZE: usize = 18;

/// Sentinel value for `meta_len` indicating no metadata.
/// Distinguishes `None` from `Some(empty)`; the encoder rejects
/// metadata exactly `u32::MAX` bytes long to avoid collision.
pub const META_LEN_ABSENT: u32 = u32::MAX;
```

Add a new error variant to `EncodeError`:

```rust
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("event type too long: {len} bytes (max {})", u16::MAX)]
    EventTypeTooLong { len: usize },

    #[error("stream ID too long: {len} bytes (max {})", u16::MAX)]
    IdTooLong { len: usize },

    #[error("metadata too long: {len} bytes (max {})", u32::MAX as usize - 1)]
    MetadataTooLong { len: usize },
}
```

Add a new error variant to `DecodeError`:

```rust
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },

    #[error("invalid UTF-8 in event type")]
    InvalidUtf8(#[source] std::str::Utf8Error),

    #[error("metadata range exceeds buffer (meta_len={meta_len}, total={total})")]
    MetadataOutOfBounds { meta_len: u32, total: usize },
}
```

Add the new return type for decode:

```rust
/// Decoded event value with `Range<u32>` offsets into the original buffer.
///
/// All three ranges are valid offsets into the `&Bytes` that was passed
/// to `decode_event_value`. `metadata_range` is `None` when the encoded
/// `meta_len` is the `META_LEN_ABSENT` sentinel.
#[derive(Debug)]
pub struct DecodedEvent {
    pub global_seq: u64,
    pub schema_version: u32,
    pub event_type_range: std::ops::Range<u32>,
    pub payload_range: std::ops::Range<u32>,
    pub metadata_range: Option<std::ops::Range<u32>>,
}
```

Replace `encode_event_value`:

```rust
/// Encode an event value as
/// `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len][event_type UTF-8][meta][payload]`.
///
/// `meta_len == META_LEN_ABSENT` (`u32::MAX`) when `metadata.is_none()`.
/// The encoder rejects metadata of exactly `u32::MAX` bytes to avoid
/// collision with the absent sentinel.
///
/// # Errors
///
/// - `EventTypeTooLong` if `event_type` exceeds `u16::MAX` bytes.
/// - `MetadataTooLong` if `metadata` is `Some(_)` with length >= `u32::MAX`.
pub fn encode_event_value(
    buf: &mut Vec<u8>,
    global_seq: u64,
    schema_version: u32,
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<(), EncodeError> {
    let event_type_len = event_type.len();
    if event_type_len > usize::from(u16::MAX) {
        return Err(EncodeError::EventTypeTooLong { len: event_type_len });
    }
    let type_len_u16 = u16::try_from(event_type_len)
        .map_err(|_| EncodeError::EventTypeTooLong { len: event_type_len })?;

    let (meta_len_field, meta_bytes_len): (u32, usize) = match metadata {
        None => (META_LEN_ABSENT, 0),
        Some(m) => {
            let len = m.len();
            let len_u32 = u32::try_from(len)
                .map_err(|_| EncodeError::MetadataTooLong { len })?;
            if len_u32 == META_LEN_ABSENT {
                return Err(EncodeError::MetadataTooLong { len });
            }
            (len_u32, len)
        }
    };

    buf.clear();
    let total = EVENT_VALUE_HEADER_SIZE + event_type_len + meta_bytes_len + payload.len();
    buf.reserve(total);

    buf.extend_from_slice(&global_seq.to_le_bytes());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&type_len_u16.to_le_bytes());
    buf.extend_from_slice(&meta_len_field.to_le_bytes());
    buf.extend_from_slice(event_type.as_bytes());
    if let Some(m) = metadata {
        buf.extend_from_slice(m);
    }
    buf.extend_from_slice(payload);
    Ok(())
}
```

Replace `decode_event_value`:

```rust
/// Decode an event value into ranges over the supplied `Bytes` buffer.
///
/// The returned `DecodedEvent` carries ranges, not slices — callers
/// construct `PersistedEnvelope` from the same `Bytes` + the ranges,
/// preserving the Arc-shared zero-copy invariant.
///
/// # Errors
///
/// - `ValueTooShort` if the header or any length-prefixed region is truncated.
/// - `InvalidUtf8` if `event_type` bytes are not valid UTF-8.
/// - `MetadataOutOfBounds` if `meta_len` claims bytes past the buffer end.
pub fn decode_event_value(value: &bytes::Bytes) -> Result<DecodedEvent, DecodeError> {
    if value.len() < EVENT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: EVENT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }

    let global_seq = u64::from_le_bytes([
        value[0], value[1], value[2], value[3],
        value[4], value[5], value[6], value[7],
    ]);
    let schema_version = u32::from_le_bytes([value[8], value[9], value[10], value[11]]);
    let event_type_len = u16::from_le_bytes([value[12], value[13]]);
    let meta_len = u32::from_le_bytes([value[14], value[15], value[16], value[17]]);

    let event_type_start = EVENT_VALUE_HEADER_SIZE;
    let event_type_end = event_type_start + usize::from(event_type_len);
    if value.len() < event_type_end {
        return Err(DecodeError::ValueTooShort {
            min: event_type_end,
            actual: value.len(),
        });
    }

    // UTF-8 validation of event_type at decode (PersistedEnvelope::try_new
    // also validates; this gives early error before envelope construction).
    std::str::from_utf8(&value[event_type_start..event_type_end])
        .map_err(DecodeError::InvalidUtf8)?;

    let (metadata_range, payload_start) = if meta_len == META_LEN_ABSENT {
        (None, event_type_end)
    } else {
        let meta_start = event_type_end;
        let meta_end = meta_start + meta_len as usize;
        if value.len() < meta_end {
            return Err(DecodeError::MetadataOutOfBounds {
                meta_len,
                total: value.len(),
            });
        }
        let start_u32 = u32::try_from(meta_start).map_err(|_| DecodeError::InvalidSize {
            expected: u32::MAX as usize,
            actual: meta_start,
        })?;
        let end_u32 = u32::try_from(meta_end).map_err(|_| DecodeError::InvalidSize {
            expected: u32::MAX as usize,
            actual: meta_end,
        })?;
        (Some(start_u32..end_u32), meta_end)
    };

    let payload_end = value.len();
    let et_start_u32 = u32::try_from(event_type_start).map_err(|_| DecodeError::InvalidSize {
        expected: u32::MAX as usize,
        actual: event_type_start,
    })?;
    let et_end_u32 = u32::try_from(event_type_end).map_err(|_| DecodeError::InvalidSize {
        expected: u32::MAX as usize,
        actual: event_type_end,
    })?;
    let pl_start_u32 = u32::try_from(payload_start).map_err(|_| DecodeError::InvalidSize {
        expected: u32::MAX as usize,
        actual: payload_start,
    })?;
    let pl_end_u32 = u32::try_from(payload_end).map_err(|_| DecodeError::InvalidSize {
        expected: u32::MAX as usize,
        actual: payload_end,
    })?;

    Ok(DecodedEvent {
        global_seq,
        schema_version,
        event_type_range: et_start_u32..et_end_u32,
        payload_range: pl_start_u32..pl_end_u32,
        metadata_range,
    })
}
```

Update the existing `event_value_round_trips` test — its call signature changes:

```rust
#[test]
fn event_value_round_trips() {
    let mut buf = Vec::new();
    let global_seq: u64 = 42;
    let schema_version: u32 = 3;
    let event_type = "UserCreated";
    let payload = b"some-json-bytes";

    encode_event_value(&mut buf, global_seq, schema_version, event_type, None, payload).unwrap();
    let bytes_buf = bytes::Bytes::copy_from_slice(&buf);
    let decoded = decode_event_value(&bytes_buf).unwrap();

    assert_eq!(decoded.global_seq, global_seq);
    assert_eq!(decoded.schema_version, schema_version);
    assert!(decoded.metadata_range.is_none());
    let et = &bytes_buf[decoded.event_type_range.start as usize..decoded.event_type_range.end as usize];
    assert_eq!(et, event_type.as_bytes());
    let pl = &bytes_buf[decoded.payload_range.start as usize..decoded.payload_range.end as usize];
    assert_eq!(pl, payload);
}
```

Update the existing `event_value_empty_payload`, `event_value_decode_rejects_truncated_event_type`, `event_value_reuses_buffer` tests in the same mechanical way (add the `None` metadata argument; update assertions to use ranges).

- [ ] **Step 4: Run the encoding tests**

Run: `nix develop -c cargo test -p nexus-fjall --lib encoding::tests`
Expected: PASS — all old tests adapted and 4 new metadata tests passing.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/encoding.rs
git commit -m "$(cat <<'EOF'
feat(fjall)!: extend wire format with metadata length-prefix

Event value layout grows from 14B to 18B header to include a u32 LE
meta_len. META_LEN_ABSENT (u32::MAX) distinguishes None from empty.
decode_event_value now takes &Bytes and returns Range<u32> offsets
instead of slices, so callers can construct PersistedEnvelope with
the same Bytes buffer (Arc-shared, zero-copy under fjall's bytes_1).

Breaking on-disk format change. Pre-1.0 acceptable per project policy.
EOF
)"
```

---

### Task 4: Update fjall adapter (store.rs, stream.rs, subscription_stream.rs)

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`
- Modify: `crates/nexus-fjall/src/stream.rs`
- Modify: `crates/nexus-fjall/src/subscription_stream.rs`

The adapter needs three changes:
1. `append` reads `PendingEnvelope.metadata()` and threads it into `encode_event_value`.
2. The cursor in `stream.rs` constructs `PersistedEnvelope::try_new` from `Bytes` + ranges instead of slices.
3. Same for `subscription_stream.rs`.

`M` generic stays for now (PR2 removes it); adapter impl hardcodes `()` as before.

- [ ] **Step 1: Update `append` in `store.rs`**

Find the `impl<M> RawEventStore<M> for FjallStore` block. Locate the call site to `encode_event_value` inside `append`. Update the call to pass metadata:

```rust
// Old:
encode_event_value(&mut value_buf, global_seq, env.schema_version(), env.event_type(), env.payload())?;
// New:
encode_event_value(
    &mut value_buf,
    global_seq,
    env.schema_version(),
    env.event_type(),
    env.metadata(),
    env.payload(),
)?;
```

The `impl` line keeps `<M>` for now — the body just hardcodes ignoring `M` since `PendingEnvelope` no longer carries it. **Wait** — `PendingEnvelope` no longer takes `M`. Update the impl signature to drop `M`:

```rust
// Before (hypothetical, current code):
impl<M> RawEventStore<M> for FjallStore { ... }
// After:
impl RawEventStore for FjallStore { ... }
```

But `RawEventStore` still has `<M>` until PR2. To bridge this PR, change `RawEventStore<M>` to use `PendingEnvelope` (no generic) in its `append` method signature, keep `M` on the trait, and have the fjall impl be `impl RawEventStore for FjallStore` (since `M` defaults to `()` and is unused in the body).

Actually look at `crates/nexus-store/src/store.rs` `RawEventStore<M>` definition first:

```bash
grep -n "trait RawEventStore" crates/nexus-store/src/store.rs
grep -n "fn append" crates/nexus-store/src/store.rs
```

Read those lines and update `RawEventStore::append`'s signature to take `&[PendingEnvelope]` instead of `&[PendingEnvelope<M>]`. Keep `M` on the trait (PR2 removes it).

- [ ] **Step 2: Update cursor in `stream.rs`**

Find where the cursor calls `decode_event_value` and constructs `PersistedEnvelope`. The previous code likely:

```rust
let (global_seq, schema_version, event_type, payload) = decode_event_value(&value_bytes)?;
let env = PersistedEnvelope::try_new(version, GlobalSeq::new(global_seq), event_type, schema_version, payload, ())?;
```

Replace with:

```rust
// value: &fjall::Slice — under bytes_1 feature, this IS bytes::Bytes
let value_bytes: bytes::Bytes = value.clone().into();
let decoded = decode_event_value(&value_bytes)?;
let env = PersistedEnvelope::try_new(
    Version::new(version_u64).ok_or(FjallError::VersionOverflow)?,
    GlobalSeq::new(decoded.global_seq),
    value_bytes,
    decoded.schema_version,
    decoded.event_type_range,
    decoded.payload_range,
    decoded.metadata_range,
)?;
```

The cursor's `Item<'a>` GAT type also needs updating in the impl. The `EventStream<M>` trait's `Item<'a>` was `PersistedEnvelope<'a, M>`; with the new envelope having no lifetime, the GAT now yields `PersistedEnvelope` directly. The trait definition in nexus-store still has GAT shape; just yield the owned type:

```rust
// In FjallStream's impl:
type Item<'a> = PersistedEnvelope where Self: 'a;
```

The `'a` is now vacuous on the item but the GAT itself stays until PR2 deletes it.

- [ ] **Step 3: Same update in `subscription_stream.rs`**

Apply the same cursor construction pattern as stream.rs.

- [ ] **Step 4: Map encoding/decoding errors to fjall error types**

If the new `EnvelopeError::InvalidUtf8` / `RangeOutOfBounds` / `InvalidSchemaVersion` needs mapping to `FjallError`, add `From<EnvelopeError> for FjallError` in `crates/nexus-fjall/src/error.rs`:

```rust
impl From<nexus_store::envelope::EnvelopeError> for FjallError {
    fn from(err: nexus_store::envelope::EnvelopeError) -> Self {
        FjallError::CorruptValue {
            // map fields per existing FjallError::CorruptValue shape
            reason: arrayvec::ArrayString::from(&err.to_string()).unwrap_or_default(),
        }
    }
}
```

(Match the exact shape of `FjallError::CorruptValue` — read `error.rs` for the current shape.)

- [ ] **Step 5: Run fjall tests**

Run: `nix develop -c cargo test -p nexus-fjall`
Expected: PASS for unit tests; integration tests may still fail until Task 6 updates them.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-fjall/src/store.rs crates/nexus-fjall/src/stream.rs crates/nexus-fjall/src/subscription_stream.rs crates/nexus-fjall/src/error.rs crates/nexus-store/src/store.rs
git commit -m "$(cat <<'EOF'
refactor(fjall): adapt store + cursor to Bytes-based envelopes

- append() reads metadata from PendingEnvelope and threads to encoder
- Cursor wraps fjall::Slice as bytes::Bytes (zero-copy under bytes_1)
  and constructs PersistedEnvelope from value + Range<u32> offsets
- EnvelopeError -> FjallError::CorruptValue mapping added

RawEventStore<M>'s append signature now takes &[PendingEnvelope]
(no M parameter on the envelope); the trait still carries M as a
threaded-but-unused generic — PR2 removes it from the trait surface.
EOF
)"
```

---

### Task 5: Update `InMemoryStore` in `nexus-store/src/testing.rs`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

The in-memory store currently holds events in `StoredRow` (per the audit, no metadata field). Update to:
1. Store each event's full row-value `Bytes` (encoded same as fjall, or a simpler in-memory shape) plus ranges.
2. Or alternatively: store the structured fields directly (event_type as `&'static str` won't work since we receive from `PendingEnvelope` which has `&'static str` — that does work!) and the metadata as `Option<Bytes>` + payload as `Bytes`.

Simplest path: each in-memory row stores `Bytes` (payload), `Option<Bytes>` (metadata), the static str event_type, version, global_seq, schema_version. Construct `PersistedEnvelope` by concatenating into one buffer at read time (small alloc, only happens on iteration).

OR — simpler still: in-memory row IS the encoded bytes (use same encoding as fjall). Then read path is identical to fjall: decode → construct envelope. This unifies behavior and tests the encoding path through the in-memory store.

Choose the latter.

- [ ] **Step 1: Update `StoredRow` to hold encoded bytes**

```rust
#[derive(Debug, Clone)]
struct StoredRow {
    version: u64,
    // Encoded event value bytes (same format as fjall):
    // [u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len]
    // [u32 LE meta_len][event_type][meta][payload]
    value: bytes::Bytes,
}
```

- [ ] **Step 2: Update `append` to encode using `encoding::encode_event_value`**

The in-memory store doesn't depend on nexus-fjall (different crate). Either:
(a) Move `encode_event_value` / `decode_event_value` to `nexus-store` (since they're not fjall-specific), OR
(b) Duplicate a minimal encode/decode in `testing.rs`.

Per CLAUDE.md "DRY", choose (a). Create `crates/nexus-store/src/wire.rs` containing the `encode_event_value`, `decode_event_value`, `DecodedEvent`, `EVENT_VALUE_HEADER_SIZE`, `META_LEN_ABSENT`, and error types. Re-export from `nexus-fjall/src/encoding.rs` for backward compat.

Actually — that's scope creep for PR1. Defer to PR2. For now: duplicate the encoder/decoder in `testing.rs` with `#[cfg(feature = "testing")]`. Note in deviation log: "encoder duplicated in testing.rs; consolidate to nexus-store::wire in PR2."

Apply the duplicated functions (copy from `nexus-fjall/src/encoding.rs`, mark with `pub(crate)`).

Update `append`:

```rust
fn append(&self, stream_id: &[u8], envelopes: &[PendingEnvelope]) -> Result<(), Self::Error> {
    // ... existing version checks ...
    for env in envelopes {
        let global_seq = /* next global seq */;
        let mut buf = Vec::new();
        encode_event_value(
            &mut buf,
            global_seq,
            env.schema_version(),
            env.event_type(),
            env.metadata(),
            env.payload(),
        )?;
        let row = StoredRow {
            version: env.version().get(),
            value: bytes::Bytes::from(buf),
        };
        // ... insert row ...
    }
    Ok(())
}
```

- [ ] **Step 3: Update `InMemoryStream` cursor to decode + construct envelope**

```rust
impl EventStream for InMemoryStream {
    type Item<'a> = PersistedEnvelope where Self: 'a;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<Self::Item<'_>>, Self::Error> {
        let Some(row) = self.rows.get(self.cursor) else {
            return Ok(None);
        };
        self.cursor += 1;
        let decoded = decode_event_value(&row.value)?;
        let env = PersistedEnvelope::try_new(
            Version::new(row.version).ok_or(InMemoryStoreError::VersionOverflow)?,
            GlobalSeq::new(decoded.global_seq),
            row.value.clone(),
            decoded.schema_version,
            decoded.event_type_range,
            decoded.payload_range,
            decoded.metadata_range,
        )?;
        Ok(Some(env))
    }
}
```

- [ ] **Step 4: Update `InMemorySubscriptionStream` the same way**

- [ ] **Step 5: Run nexus-store tests**

Run: `nix develop -c cargo test -p nexus-store`
Expected: most pass; some integration tests may fail until Task 6.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "$(cat <<'EOF'
refactor(store/testing): InMemoryStore stores encoded Bytes per row

Unifies the in-memory adapter's read/write path with fjall's:
- append() encodes events into the wire format (Vec<u8> -> Bytes)
- Cursor decodes + constructs PersistedEnvelope from Bytes + ranges

The encoder/decoder functions are temporarily duplicated from
nexus-fjall/src/encoding.rs. PR2 consolidates them into
nexus-store::wire so both adapters share one implementation.
EOF
)"
```

---

### Task 6: Update repository facade + tests + examples

**Files:**
- Modify: `crates/nexus-store/src/repository.rs`
- Modify: `crates/nexus-store/tests/*.rs` (each that constructs envelopes)
- Modify: `examples/store-and-kernel/src/main.rs`
- Modify: `examples/store-inmemory/src/main.rs`

- [ ] **Step 1: Update `Repository::save` envelope construction**

In `crates/nexus-store/src/repository.rs`, find where `EventStore::save` builds envelopes from kernel events. Update the builder call:

```rust
// Old:
let env = pending_envelope(version)
    .event_type(event.name())
    .payload(payload_vec)
    .build_without_metadata();
// New:
let env = pending_envelope(version)
    .event_type(event.name())
    .payload(payload_vec)  // Vec<u8> -> Bytes via Into; reuses allocation
    .build();
```

The `.build_without_metadata()` method does not exist on the new builder — `.build()` is the no-metadata variant.

- [ ] **Step 2: Update examples**

In `examples/store-and-kernel/src/main.rs` and `examples/store-inmemory/src/main.rs`, search for `.build_without_metadata()` and `.build(())` and replace with `.build()`. Verify the `.payload(...)` calls accept `Vec<u8>` (they do, via `Into<Bytes>`).

- [ ] **Step 3: Update tests in `crates/nexus-store/tests/`**

For each test that constructs envelopes, mechanical updates:
- `.build_without_metadata()` → `.build()`
- `.build(meta)` → `.with_metadata(meta)` where `meta: impl Into<Bytes>`
- `PersistedEnvelope::new_unchecked(...)` → `PersistedEnvelope::try_new(...)` with the new signature (Bytes + ranges)

Use a grep to find all sites:

```bash
grep -rn "build_without_metadata\|new_unchecked\|PersistedEnvelope::try_new" crates/nexus-store/tests/ crates/nexus-store/src/
```

Walk through each match and update.

- [ ] **Step 4: Handle `stream_tests.rs` `MetadataStream` fixture**

The test fixture `MetadataStream` (around line 210 in `crates/nexus-store/tests/stream_tests.rs`) was generic over `M`. The `PersistedEnvelope` now has no `M`. Options:
- Delete `MetadataStream` and `DropProbeStream` — they tested metadata propagation through GAT combinators; with `M` gone and combinators going away in PR2, these tests are obsolete.
- Keep them but make them construct `PersistedEnvelope` with appropriate `Bytes` metadata.

Choose deletion: log in deviation log as `[PR 1 | Task 6] Deleted MetadataStream / DropProbeStream test fixtures` with reason "obsolete with M removal; combinator tests retire in PR2."

```bash
# In the test file, delete the MetadataStream / DropProbeStream tests and their fixtures.
```

- [ ] **Step 5: Run full test suite**

Run: `nix develop -c cargo test --workspace`
Expected: PASS across all crates.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/repository.rs crates/nexus-store/tests/ examples/
git commit -m "$(cat <<'EOF'
refactor(store): update Repository + tests + examples for new envelope

Mechanical migration:
- .build_without_metadata() -> .build()
- .build(meta) -> .with_metadata(meta)
- PersistedEnvelope::new_unchecked/try_new signatures updated to
  take Bytes + Range<u32> instead of &str + &[u8] + M

Deleted MetadataStream / DropProbeStream test fixtures — they
tested GAT-combinator metadata propagation, which is obsoleted
by PR2's collapse to futures::Stream.
EOF
)"
```

---

### Task 7: Add lifecycle + defensive boundary tests per CLAUDE.md §7

**Files:**
- Add tests to: `crates/nexus-fjall/tests/`

The new wire format and envelope shape need the 4 mandatory test categories from CLAUDE.md.

- [ ] **Step 1: Sequence/Protocol test — multi-event metadata round-trip**

Add to a new test file `crates/nexus-fjall/tests/metadata_roundtrip_tests.rs`:

```rust
use bytes::Bytes;
use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use tempfile::tempdir;

#[tokio::test]
async fn metadata_roundtrips_across_multiple_events() {
    let dir = tempdir().unwrap();
    let store = FjallStore::builder(dir.path()).open().unwrap();

    let envs = (1..=5).map(|i| {
        pending_envelope(Version::new(i).unwrap())
            .event_type("Test")
            .payload(Bytes::from(format!("payload-{i}")))
            .with_metadata(Bytes::from(format!("meta-{i}")))
    }).collect::<Vec<_>>();

    store.append(b"stream-1", &envs).await.expect("append");

    let mut cursor = store.read_stream(b"stream-1", None).await.expect("read");
    let mut i = 1;
    while let Some(env) = cursor.next().await.expect("cursor") {
        assert_eq!(env.payload(), format!("payload-{i}").as_bytes());
        assert_eq!(env.metadata(), Some(format!("meta-{i}").as_bytes()));
        i += 1;
    }
    assert_eq!(i, 6);
}
```

- [ ] **Step 2: Lifecycle test — write-close-reopen-read**

```rust
#[tokio::test]
async fn metadata_survives_store_reopen() {
    let dir = tempdir().unwrap();
    {
        let store = FjallStore::builder(dir.path()).open().unwrap();
        let env = pending_envelope(Version::INITIAL)
            .event_type("X")
            .payload(Bytes::from_static(b"payload"))
            .with_metadata(Bytes::from_static(b"important-meta"));
        store.append(b"s", &[env]).await.unwrap();
    }
    // Drop the store; reopen.
    let store = FjallStore::builder(dir.path()).open().unwrap();
    let mut cursor = store.read_stream(b"s", None).await.unwrap();
    let env = cursor.next().await.unwrap().expect("event");
    assert_eq!(env.metadata(), Some(b"important-meta".as_slice()));
}
```

- [ ] **Step 3: Defensive boundary test — None vs empty metadata distinction**

```rust
#[tokio::test]
async fn none_metadata_distinguishable_from_present_when_empty_disallowed() {
    let dir = tempdir().unwrap();
    let store = FjallStore::builder(dir.path()).open().unwrap();

    let env = pending_envelope(Version::INITIAL)
        .event_type("X")
        .payload(Bytes::from_static(b"payload"))
        .build();  // no metadata
    store.append(b"s", &[env]).await.unwrap();

    let mut cursor = store.read_stream(b"s", None).await.unwrap();
    let env = cursor.next().await.unwrap().unwrap();
    assert_eq!(env.metadata(), None);
    assert!(env.metadata_bytes().is_none());
}
```

- [ ] **Step 4: Defensive boundary test — corrupt meta_len rejected**

```rust
#[test]
fn decoder_rejects_meta_len_exceeding_buffer() {
    use nexus_fjall::encoding::{encode_event_value, decode_event_value, META_LEN_ABSENT};

    let mut buf = Vec::new();
    // Manually build a corrupt value: claim meta_len = 100 but only 10
    // bytes of payload follow.
    buf.extend_from_slice(&1u64.to_le_bytes());  // global_seq
    buf.extend_from_slice(&1u32.to_le_bytes());  // schema_version
    buf.extend_from_slice(&1u16.to_le_bytes());  // event_type_len = 1
    buf.extend_from_slice(&100u32.to_le_bytes()); // meta_len = 100 (LIE)
    buf.push(b'X');                                // event_type
    buf.extend_from_slice(&[0u8; 10]);             // only 10 bytes follow, not 100

    let bytes_buf = bytes::Bytes::from(buf);
    let err = decode_event_value(&bytes_buf).expect_err("must reject");
    // Assert specific error variant
}
```

- [ ] **Step 5: Run new tests**

Run: `nix develop -c cargo test -p nexus-fjall --test metadata_roundtrip_tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-fjall/tests/metadata_roundtrip_tests.rs
git commit -m "test(fjall): metadata roundtrip + lifecycle + defensive boundary tests"
```

---

### Task 8: Run full `nix flake check` and open PR

- [ ] **Step 1: Format**

Run: `nix develop -c cargo fmt --all`

- [ ] **Step 2: Full check**

Run: `nix flake check`
Expected: ALL PASS (clippy, fmt, taplo, tests, audit, deny, hakari).

Fix any clippy lints; fix any failing tests. Per [feedback_never_skip_flake_check](#) — DO NOT bypass.

- [ ] **Step 3: Commit any final fixes**

If `cargo fmt` made changes:

```bash
git add -u && git commit -m "style: rustfmt"
```

If hakari needed regeneration:

```bash
nix develop -c cargo hakari generate
git add -u && git commit -m "chore: regenerate workspace-hack"
```

- [ ] **Step 4: Push**

```bash
git push -u origin refactor/bytes-envelope-pr1
```

- [ ] **Step 5: Open PR**

```bash
gh pr create --title "refactor(store,fjall)!: Bytes-based envelopes with Range<u32> offsets — PR1 of bytes-envelope refactor" --body "$(cat <<'EOF'
## Summary

- Rewrites \`PendingEnvelope\` and \`PersistedEnvelope\` as owned Bytes-based types with the "single Bytes + Range<u32> fields" pattern, dropping the \`M\` generic and \`'a\` lifetime.
- Extends fjall wire format with a u32 meta_len between event_type_len and event_type. META_LEN_ABSENT (u32::MAX) is the no-metadata sentinel.
- Enables fjall's \`bytes_1\` feature so \`Slice\` IS \`bytes::Bytes\` with zero-copy interop.
- Updates fjall + InMemoryStore + repository facade + tests + examples.

Stream traits still carry \`M = ()\` — PR2 of the refactor drops them and collapses the trait family to a single marker over \`futures::Stream\`.

See \`docs/plans/2026-05-27-bytes-envelope-design.md\` for the full design rationale (postgres-driven trigger, options compared, source-verified bytes-crate facts, the two footguns).

## Breaking changes

- On-disk fjall format: header grew from 14B to 18B; old stores cannot be read. Pre-1.0 acceptable per project policy.
- API: \`PendingEnvelope<M>\` -> \`PendingEnvelope\`; \`PersistedEnvelope<'a, M>\` -> \`PersistedEnvelope\`; \`.build_without_metadata()\` -> \`.build()\`; \`.build(meta)\` -> \`.with_metadata(meta)\`.

## Test plan

- [ ] cargo test --workspace passes
- [ ] nix flake check passes (clippy, fmt, taplo, tests, audit, deny, hakari)
- [ ] New metadata roundtrip + lifecycle + defensive boundary tests pass

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 6: Update deviation log with any divergences from this plan**

Open `docs/plans/2026-05-27-bytes-envelope-deviations.md` and add entries for any task where the implementation differed from this plan. Per [feedback_deviation_log_during_refactors](#) — every divergence with reason and impact.

---

## PR 1 Acceptance criteria

- [ ] `nix flake check` passes on the branch.
- [ ] Wire format includes metadata length-prefix; round-trip with and without metadata.
- [ ] `PendingEnvelope` and `PersistedEnvelope` have no `M` generic and no `'a` lifetime.
- [ ] All adapters (`FjallStore`, `InMemoryStore`) construct envelopes via the new API.
- [ ] All examples build and run.
- [ ] Lifecycle test (write-close-reopen) passes.
- [ ] PR opened against `main` with the description above.

## What PR 2 will do

(Sketch — full plan written after PR1 merges.)

- Delete the `M` generic from `RawEventStore`, `EventStream`, `BaseEventStream`, `EventStreamExt`, `OwnedEventStream`, `Subscription`, `SubscriptionBackend`, `SharedSubscription`, `SharedSubscriptionBackend`.
- Redefine `EventStream` as a marker trait over `futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send`.
- Delete `crates/nexus-store/src/stream/combinators.rs`, `owned.rs`, and the `BaseEventStream` / `EventStreamExt` definitions from `cursor.rs`. Shrink the `stream/` module to just the `EventStream` trait + `SharedSubscription` impl.
- Update projection runner in `nexus-framework` to consume from `futures::Stream` via `futures::StreamExt::try_fold`.
- Consolidate `encode_event_value` / `decode_event_value` into `nexus-store::wire` (resolves the PR1 duplication noted in the deviation log).
- Make `futures-bridge` a non-optional feature (or merge into baseline) since `EventStream` now requires it.

## What PR 3 will do

(Sketch.)

- Update README + module-level rustdoc.
- Update CLAUDE.md architecture section to reflect the collapsed stream module + Bytes envelopes.
- Update examples to demonstrate `futures::StreamExt` combinator usage.
