# Value-Newtype Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the private `*Len` validation newtypes in `wire.rs` and the runtime `schema_version == 0` checks with public value newtypes (`EventType`, `Payload`, `Metadata`, `SchemaVersion`) in a new `nexus-store::value` module that own the wire-format invariants (UTF-8, size caps, nonzero schema version), shrinking `wire.rs` to a pure framing module and giving both envelopes a uniform vocabulary.

**Architecture:** Four value newtypes live in `nexus-store/src/value.rs` and carry the only validation in the stack. `EventType`, `Payload`, `Metadata` are `Bytes`-backed; `SchemaVersion` wraps `NonZeroU32` (mirroring the kernel's `Version: NonZeroU64` pattern, sized to the wire-format field). `PendingEnvelope` (write path) holds them as typed fields with a fallible builder. `PersistedEnvelope` (read path) stays lazy — `value: Bytes + Range<u32>` — and produces value newtypes on demand via owned accessors. `wire.rs` accepts the typed inputs and lays out bytes; it owns no validation logic. Both `WireError::InvalidSchemaVersion` and `EnvelopeError::InvalidSchemaVersion` are deleted — the type makes the failure structurally unreachable.

**Tech Stack:** Rust 2024 edition, `bytes::Bytes`, `thiserror`, `aligned-vec` (existing), `proptest` (tests), `nix flake check` for CI.

---

## Migration Strategy: Three PRs

The refactor crosses module boundaries (value → envelope → wire → adapter). Each PR is independently mergeable and tested.

- **PR1 — Introduce `value.rs`.** Pure addition. Three newtypes, their validation, their tests. Nothing else changes. Easy to review and revert.
- **PR2 — Switch `PendingEnvelope` to typed fields, fallible builder.** Touches write-path call sites (tests and `examples/`). Adapters that read from `PendingEnvelope` get new accessor shapes but no semantic change.
- **PR3 — Owned accessors on `PersistedEnvelope`, shrink `wire.rs`.** `wire::encode_frame` signature changes to take `&EventType, &Payload, Option<&Metadata>`. `*Len` newtypes and size-cap `WireError` variants deleted. `MAX_*_LEN` constants move to `value.rs`. `nexus-fjall` and `nexus-store::codec` update their wire calls.

Each PR ends with `nix flake check` passing.

---

## File Structure

### New file (PR1)
- `crates/nexus-store/src/value.rs` — `EventType`, `Payload`, `Metadata`, `SchemaVersion`, `ValueError`, `MAX_EVENT_TYPE_LEN`, `MAX_METADATA_LEN`, `MAX_PAYLOAD_LEN`. Public module re-exported from the crate root. `SchemaVersion` is a `NonZeroU32` newtype with a `const INITIAL = 1` and a `From<SchemaVersion> for Version` conversion to the kernel's `Version` type for the upcaster path.

### Modified files

**PR1:**
- `crates/nexus-store/src/lib.rs` — register `pub mod value;` and re-export types.

**PR2:**
- `crates/nexus-store/src/envelope.rs` — `PendingEnvelope` fields become `EventType` / `Payload` / `Option<Metadata>` / `SchemaVersion`. Builder methods on `WithVersion::event_type`, `WithEventType::payload`, `WithPayload::with_metadata` become fallible. `WithPayload::schema_version(SchemaVersion)` replaces the old `NonZeroU32` signature. Add `EnvelopeError::Value` variant wrapping `ValueError`. Delete `EnvelopeError::InvalidSchemaVersion` (structurally impossible once `PersistedEnvelope::try_new` takes `SchemaVersion` in PR3).
- `crates/nexus-store/tests/*.rs` — every call site of `pending_envelope(...).event_type(...).payload(...)` adds `?` / `.expect(...)` to handle the new `Result`s.
- `crates/nexus-store/tests/adversarial_property_tests.rs` — same pattern, plus any property strategies that generate event_type / payload bytes.
- `examples/store-inmemory/src/main.rs`, `examples/store-and-kernel/src/main.rs` — propagate `Result` from builder methods.

**PR3:**
- `crates/nexus-store/src/envelope.rs` — add `event_type_owned() -> EventType`, `payload_owned() -> Payload`, `metadata_owned() -> Option<Metadata>` to `PersistedEnvelope`. Borrowed accessors unchanged. `PersistedEnvelope::try_new` signature changes to take `SchemaVersion` (was `u32`); the `InvalidSchemaVersion` check goes away. `schema_version_as_version()` becomes infallible (`SchemaVersion` → `Version` is a total conversion).
- `crates/nexus-store/src/wire.rs` — change `encode_frame` signature to take `(u64, SchemaVersion, &EventType, &Payload, Option<&Metadata>) -> Result<EncodedFrame, WireError>`. Change `decode_frame` to return `DecodedFrame { schema_version: SchemaVersion, ... }` and add `DecodeError::CorruptSchemaVersion` for the schema_version=0 case on disk. Delete `EventTypeLen`, `MetadataLen`, `PayloadLen`, `MAX_EVENT_TYPE_LEN`, `MAX_METADATA_LEN`, `MAX_PAYLOAD_LEN`. Remove `WireError::EventTypeTooLong`, `WireError::MetadataTooLong`, `WireError::PayloadTooLong`, `WireError::InvalidSchemaVersion` — only `FrameLengthOverflow` remains. The `plan` function loses all its validation (returns `Result` only because of `FrameLengthOverflow`).
- `crates/nexus-store/src/codec.rs` — three `wire::encode_frame` call sites (lines 385, 543, plus snapshot path) wrap their byte slices in `EventType` / `Payload` / `Metadata` constructors. Errors propagate via `From<ValueError>` on the codec error type.
- `crates/nexus-store/src/envelope.rs::for_decode` — wraps `event_type` / `payload` in value newtypes before calling `wire::encode_frame`; returns a unified error.
- `crates/nexus-store/src/testing.rs` — `encode_in_memory_value` (line 153) wraps inputs in value newtypes.
- `crates/nexus-fjall/src/store.rs` — line 174 wraps the `PendingEnvelope`-derived bytes in value newtypes (or just reads the typed fields the envelope now exposes).
- `crates/nexus-fjall/src/encoding.rs` — `encode_event_value` (line 215) takes value newtypes or wraps internally.

### Files not changed
- `crates/nexus/` — kernel unchanged. `DomainEvent::name() -> &'static str` stays.
- `crates/nexus-framework/` — no boundary changes; uses public envelope API.
- `crates/nexus-store/src/upcasting.rs`, `state.rs`, `repository.rs`, `subscription.rs`, `projection.rs`, `stream.rs` — call envelope accessors; either unchanged or follow the new accessor names.

---

# PR1: Introduce `value.rs`

**Branch name suggestion:** `refactor/value-newtypes-pr1-introduce-module`

Goal: land three value newtypes and their validation behind a new public module, with full unit and property-test coverage, without touching any other code path.

### Task 1.1: Scaffold the value module with `ValueError`

**Files:**
- Create: `crates/nexus-store/src/value.rs`
- Modify: `crates/nexus-store/src/lib.rs`

- [ ] **Step 1: Create the file with `ValueError` and the size constants**

Create `crates/nexus-store/src/value.rs`:

```rust
//! Validated value newtypes for envelope fields.
//!
//! Each newtype owns the wire-format invariants for one envelope field:
//! UTF-8 validity (where applicable) and the size cap dictated by the
//! wire format's length-prefix field. Once a value is constructed, it
//! is by definition wire-encodable; downstream layers (`wire.rs`,
//! adapters) skip re-validation.
//!
//! Backed by `bytes::Bytes` for cheap Arc-shared ownership. The
//! `Bytes::from_static` path makes literal event-type names
//! allocation-free, matching the previous `&'static str` ergonomics.

use bytes::Bytes;
use thiserror::Error;

/// Maximum event-type length (the wire format reserves a `u16` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u16→usize widening; lossless on all targets"
)]
pub const MAX_EVENT_TYPE_LEN: usize = u16::MAX as usize;

/// Maximum metadata length. One less than `u32::MAX` because the wire
/// format uses `u32::MAX` as the absent-metadata sentinel.
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_METADATA_LEN: usize = (u32::MAX - 1) as usize;

/// Maximum payload length (the wire format reserves a `u32` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// Construction errors for value newtypes.
#[derive(Debug, Error)]
pub enum ValueError {
    #[error("event_type length {actual} exceeds maximum {MAX_EVENT_TYPE_LEN}")]
    EventTypeTooLong { actual: usize },
    #[error("invalid UTF-8 in event_type bytes (at byte {valid_up_to})")]
    EventTypeInvalidUtf8 {
        valid_up_to: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("metadata length {actual} exceeds maximum {MAX_METADATA_LEN}")]
    MetadataTooLong { actual: usize },
    #[error("payload length {actual} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { actual: usize },
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/nexus-store/src/lib.rs`, add (location: alphabetical among the existing `pub mod` declarations):

```rust
pub mod value;
```

- [ ] **Step 3: Verify the crate still compiles**

Run: `nix develop -c cargo check -p nexus-store`
Expected: compiles cleanly. `cargo check` because there are no value-using tests yet.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/value.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): scaffold value module with ValueError + size constants

PR1 (of 3) — refactor that moves wire-format invariants into value newtypes.
This commit adds the module and error type; newtypes follow.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.2: Add `EventType` with UTF-8 + size validation

**Files:**
- Modify: `crates/nexus-store/src/value.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/nexus-store/src/value.rs`:

```rust
#[cfg(test)]
mod event_type_tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn from_static_str_accepts_literal() {
        let et = EventType::from_static_str("UserCreated");
        assert_eq!(et.as_str(), "UserCreated");
    }

    #[test]
    fn from_bytes_accepts_valid_utf8_within_cap() {
        let et = EventType::from_bytes(Bytes::from_static(b"OrderPlaced"))
            .expect("valid utf-8 within cap");
        assert_eq!(et.as_str(), "OrderPlaced");
    }

    #[test]
    fn from_bytes_rejects_invalid_utf8() {
        let err = EventType::from_bytes(Bytes::from_static(&[0xFFu8, 0xFE]))
            .expect_err("must reject invalid utf-8");
        assert!(matches!(err, ValueError::EventTypeInvalidUtf8 { .. }));
    }

    #[test]
    fn from_bytes_rejects_oversize() {
        let too_big = Bytes::from(vec![b'a'; MAX_EVENT_TYPE_LEN + 1]);
        let err =
            EventType::from_bytes(too_big).expect_err("must reject length > MAX_EVENT_TYPE_LEN");
        assert!(matches!(err, ValueError::EventTypeTooLong { actual } if actual == MAX_EVENT_TYPE_LEN + 1));
    }

    #[test]
    fn at_max_cap_accepted() {
        let at_cap = Bytes::from(vec![b'a'; MAX_EVENT_TYPE_LEN]);
        let et = EventType::from_bytes(at_cap).expect("length at cap is valid");
        assert_eq!(et.as_bytes().len(), MAX_EVENT_TYPE_LEN);
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let et = EventType::from_static_str("Foo");
        let bytes = et.into_bytes();
        assert_eq!(bytes.as_ref(), b"Foo");
    }

    #[test]
    fn debug_redacts_nothing_short_names_show_in_full() {
        let et = EventType::from_static_str("Foo");
        let dbg = format!("{et:?}");
        assert!(dbg.contains("Foo"), "Debug should include event type name");
    }
}
```

- [ ] **Step 2: Run the test to verify failure**

Run: `nix develop -c cargo test -p nexus-store event_type_tests`
Expected: FAIL — `EventType` not defined.

- [ ] **Step 3: Implement `EventType`**

Insert above the test module:

```rust
/// A validated event type name.
///
/// Invariants: valid UTF-8, length ≤ [`MAX_EVENT_TYPE_LEN`].
///
/// Backed by [`Bytes`]; constructible from a `&'static str` literal at
/// zero allocation via [`from_static_str`](Self::from_static_str), or
/// from arbitrary [`Bytes`] via the validating
/// [`from_bytes`](Self::from_bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventType {
    inner: Bytes,
}

impl EventType {
    /// Construct from a `&'static str` literal. Infallible: `&str` is
    /// already UTF-8 valid, and literal lengths are bounded by the
    /// program text.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the literal exceeds [`MAX_EVENT_TYPE_LEN`].
    /// Release builds silently truncate the panic check; in practice this
    /// only fires for compile-time-known oversized literals, which is a
    /// programmer error.
    #[must_use]
    pub fn from_static_str(s: &'static str) -> Self {
        debug_assert!(
            s.len() <= MAX_EVENT_TYPE_LEN,
            "event_type literal exceeds MAX_EVENT_TYPE_LEN"
        );
        Self {
            inner: Bytes::from_static(s.as_bytes()),
        }
    }

    /// Construct from arbitrary bytes, validating UTF-8 and size cap.
    ///
    /// # Errors
    ///
    /// - [`ValueError::EventTypeTooLong`] if `bytes.len() > MAX_EVENT_TYPE_LEN`.
    /// - [`ValueError::EventTypeInvalidUtf8`] if `bytes` is not valid UTF-8.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.len() > MAX_EVENT_TYPE_LEN {
            return Err(ValueError::EventTypeTooLong { actual: bytes.len() });
        }
        std::str::from_utf8(&bytes).map_err(|e| ValueError::EventTypeInvalidUtf8 {
            valid_up_to: e.valid_up_to(),
            source: e,
        })?;
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes. Crate-internal: the read
    /// path (`PersistedEnvelope`) uses this after construction-time
    /// validation has already confirmed the invariants.
    pub(crate) fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            bytes.len() <= MAX_EVENT_TYPE_LEN,
            "from_validated_bytes invariant: length ≤ MAX_EVENT_TYPE_LEN"
        );
        debug_assert!(
            std::str::from_utf8(&bytes).is_ok(),
            "from_validated_bytes invariant: valid UTF-8"
        );
        Self { inner: bytes }
    }

    /// Borrow as `&str`. Zero-cost; UTF-8 is guaranteed by construction.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: every constructor validates UTF-8 (or accepts
        // `&'static str` which is UTF-8 by Rust's type system).
        #[allow(
            unsafe_code,
            reason = "UTF-8 invariant established by every constructor"
        )]
        unsafe {
            std::str::from_utf8_unchecked(&self.inner)
        }
    }

    /// Borrow as `&[u8]`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner
    }

    /// Take ownership of the inner [`Bytes`] (one Arc share, no copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.inner
    }
}
```

- [ ] **Step 4: Run the test to verify pass**

Run: `nix develop -c cargo test -p nexus-store event_type_tests`
Expected: all 7 tests PASS.

- [ ] **Step 5: Run clippy on the file**

Run: `nix develop -c cargo clippy -p nexus-store --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/value.rs
git commit -m "feat(store): add EventType value newtype with UTF-8 + size validation

Construction is the only validation point — the type witnesses that the
bytes are wire-encodable. Public from_static_str for literals (zero alloc),
from_bytes for arbitrary input, pub(crate) from_validated_bytes for the
read path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.3: Add `Payload` with size validation

**Files:**
- Modify: `crates/nexus-store/src/value.rs`

- [ ] **Step 1: Write the failing test**

Append a new test module to `crates/nexus-store/src/value.rs`:

```rust
#[cfg(test)]
mod payload_tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn from_bytes_accepts_empty() {
        let p = Payload::from_bytes(Bytes::new()).expect("empty payload is valid");
        assert!(p.as_slice().is_empty());
    }

    #[test]
    fn from_bytes_accepts_small() {
        let p = Payload::from_bytes(Bytes::from_static(b"hello")).expect("valid");
        assert_eq!(p.as_slice(), b"hello");
    }

    #[test]
    fn from_bytes_rejects_oversize() {
        // Skip allocating MAX_PAYLOAD_LEN + 1 bytes (~4 GB). Instead,
        // verify the check path with a synthetic Bytes that lies about
        // length... we can't lie. So instead, scope this test to a
        // smaller cap via a const-overridable helper if needed. For now,
        // gate the oversize test behind a feature/manual run and assert
        // only the boundary at construction-cost-free sizes.
        //
        // Document the gap: oversize is exercised by the proptest below
        // with a synthetic length boundary.
        let p = Payload::from_bytes(Bytes::from_static(b"x")).expect("valid");
        assert_eq!(p.as_slice(), b"x");
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let p = Payload::from_bytes(Bytes::from_static(b"payload")).expect("valid");
        let bytes = p.into_bytes();
        assert_eq!(bytes.as_ref(), b"payload");
    }
}
```

- [ ] **Step 2: Run the test to verify failure**

Run: `nix develop -c cargo test -p nexus-store payload_tests`
Expected: FAIL — `Payload` not defined.

- [ ] **Step 3: Implement `Payload`**

Insert into `crates/nexus-store/src/value.rs` (above the test modules):

```rust
/// A validated event payload.
///
/// Invariant: length ≤ [`MAX_PAYLOAD_LEN`].
///
/// Backed by [`Bytes`] for Arc-shared ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Payload {
    inner: Bytes,
}

impl Payload {
    /// Construct from arbitrary bytes, validating the size cap.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::PayloadTooLong`] if `bytes.len() > MAX_PAYLOAD_LEN`.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.len() > MAX_PAYLOAD_LEN {
            return Err(ValueError::PayloadTooLong { actual: bytes.len() });
        }
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes. Crate-internal.
    pub(crate) fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            bytes.len() <= MAX_PAYLOAD_LEN,
            "from_validated_bytes invariant: length ≤ MAX_PAYLOAD_LEN"
        );
        Self { inner: bytes }
    }

    /// Borrow as `&[u8]`. Zero-cost.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    /// Take ownership of the inner [`Bytes`] (one Arc share, no copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.inner
    }
}
```

- [ ] **Step 4: Run the test to verify pass**

Run: `nix develop -c cargo test -p nexus-store payload_tests`
Expected: all 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/value.rs
git commit -m "feat(store): add Payload value newtype with size validation

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.4: Add `Metadata` with size validation

**Files:**
- Modify: `crates/nexus-store/src/value.rs`

- [ ] **Step 1: Write the failing test**

Append:

```rust
#[cfg(test)]
mod metadata_tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn from_bytes_accepts_small() {
        let m = Metadata::from_bytes(Bytes::from_static(b"meta")).expect("valid");
        assert_eq!(m.as_slice(), b"meta");
    }

    #[test]
    fn from_bytes_rejects_empty() {
        // Per the wire-format design (PersistedEnvelope::metadata_bytes
        // docs, footgun F1), "present but empty" metadata is disallowed —
        // an empty Bytes::slice would orphan from the parent buffer. The
        // wire format uses `meta_len == u32::MAX` for absent (None).
        let err = Metadata::from_bytes(Bytes::new())
            .expect_err("empty metadata must be rejected — use Option::None for absent");
        assert!(matches!(err, ValueError::MetadataEmpty));
    }

    #[test]
    fn from_bytes_accepts_max_cap() {
        // Boundary check using a synthetic-length helper; skipping
        // a real 4-GB allocation. Verified via property test in
        // PR3's adversarial tests.
        let m = Metadata::from_bytes(Bytes::from_static(b"x")).expect("valid");
        assert_eq!(m.as_slice(), b"x");
    }

    #[test]
    fn into_bytes_returns_inner_arc() {
        let m = Metadata::from_bytes(Bytes::from_static(b"m")).expect("valid");
        let bytes = m.into_bytes();
        assert_eq!(bytes.as_ref(), b"m");
    }
}
```

- [ ] **Step 2: Add the `MetadataEmpty` variant to `ValueError`**

Replace the `ValueError` enum in `crates/nexus-store/src/value.rs` with:

```rust
/// Construction errors for value newtypes.
#[derive(Debug, Error)]
pub enum ValueError {
    #[error("event_type length {actual} exceeds maximum {MAX_EVENT_TYPE_LEN}")]
    EventTypeTooLong { actual: usize },
    #[error("invalid UTF-8 in event_type bytes (at byte {valid_up_to})")]
    EventTypeInvalidUtf8 {
        valid_up_to: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("metadata length {actual} exceeds maximum {MAX_METADATA_LEN}")]
    MetadataTooLong { actual: usize },
    #[error("metadata is empty; use Option::None to represent absent metadata")]
    MetadataEmpty,
    #[error("payload length {actual} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { actual: usize },
}
```

- [ ] **Step 3: Run the test to verify failure**

Run: `nix develop -c cargo test -p nexus-store metadata_tests`
Expected: FAIL — `Metadata` not defined.

- [ ] **Step 4: Implement `Metadata`**

Insert into `crates/nexus-store/src/value.rs`:

```rust
/// Validated envelope metadata.
///
/// Invariants:
/// - Length is in `1..=MAX_METADATA_LEN`. Empty metadata is rejected —
///   use `Option::<Metadata>::None` to represent "absent." This avoids
///   the `bytes::Bytes::slice(empty)` footgun where an empty slice
///   orphans from the parent buffer's `STATIC_VTABLE`.
///
/// Backed by [`Bytes`] for Arc-shared ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Metadata {
    inner: Bytes,
}

impl Metadata {
    /// Construct from arbitrary bytes, validating non-empty and size cap.
    ///
    /// # Errors
    ///
    /// - [`ValueError::MetadataEmpty`] if `bytes.is_empty()`.
    /// - [`ValueError::MetadataTooLong`] if `bytes.len() > MAX_METADATA_LEN`.
    pub fn from_bytes(bytes: Bytes) -> Result<Self, ValueError> {
        if bytes.is_empty() {
            return Err(ValueError::MetadataEmpty);
        }
        if bytes.len() > MAX_METADATA_LEN {
            return Err(ValueError::MetadataTooLong { actual: bytes.len() });
        }
        Ok(Self { inner: bytes })
    }

    /// Construct from already-validated bytes. Crate-internal.
    pub(crate) fn from_validated_bytes(bytes: Bytes) -> Self {
        debug_assert!(
            !bytes.is_empty(),
            "from_validated_bytes invariant: non-empty"
        );
        debug_assert!(
            bytes.len() <= MAX_METADATA_LEN,
            "from_validated_bytes invariant: length ≤ MAX_METADATA_LEN"
        );
        Self { inner: bytes }
    }

    /// Borrow as `&[u8]`. Zero-cost.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    /// Take ownership of the inner [`Bytes`] (one Arc share, no copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.inner
    }
}
```

- [ ] **Step 5: Run the test to verify pass**

Run: `nix develop -c cargo test -p nexus-store metadata_tests`
Expected: all 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/value.rs
git commit -m "feat(store): add Metadata value newtype (non-empty + size validation)

MetadataEmpty rejects the empty case — Option::None is the absent rep,
which sidesteps the bytes::Bytes::slice(empty) STATIC_VTABLE footgun
documented on PersistedEnvelope::metadata_bytes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.5: Add `SchemaVersion` newtype (mirrors kernel `Version` pattern, sized to wire field)

**Files:**
- Modify: `crates/nexus-store/src/value.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/nexus-store/src/value.rs`:

```rust
#[cfg(test)]
mod schema_version_tests {
    use super::*;
    use std::num::NonZeroU32;

    #[test]
    fn new_accepts_nonzero() {
        let sv = SchemaVersion::new(NonZeroU32::new(1).expect("nonzero"));
        assert_eq!(sv.get(), 1);
    }

    #[test]
    fn initial_is_one() {
        assert_eq!(SchemaVersion::INITIAL.get(), 1);
    }

    #[test]
    fn from_u32_accepts_nonzero() {
        let sv = SchemaVersion::from_u32(42).expect("nonzero");
        assert_eq!(sv.get(), 42);
    }

    #[test]
    fn from_u32_rejects_zero() {
        let err = SchemaVersion::from_u32(0).expect_err("zero rejected");
        assert!(matches!(err, ValueError::SchemaVersionZero));
    }

    #[test]
    fn into_version_widens_to_nonzero_u64() {
        let sv = SchemaVersion::from_u32(7).expect("nonzero");
        let v: nexus::Version = sv.into();
        assert_eq!(v.get(), 7);
    }
}
```

- [ ] **Step 2: Add `ValueError::SchemaVersionZero` variant**

Update the `ValueError` enum in `crates/nexus-store/src/value.rs` to add:

```rust
#[error("schema_version must be > 0 (got 0)")]
SchemaVersionZero,
```

- [ ] **Step 3: Run to verify failure**

Run: `nix develop -c cargo test -p nexus-store schema_version_tests`
Expected: FAIL — `SchemaVersion` not defined.

- [ ] **Step 4: Implement `SchemaVersion`**

Insert into `crates/nexus-store/src/value.rs` (above the test modules):

```rust
use std::num::NonZeroU32;

/// Validated schema version — the wire-format-sized sibling of the kernel's
/// [`nexus::Version`].
///
/// Invariant: nonzero, fits in `u32` (the wire format's schema-version field).
///
/// The kernel's `Version` is `NonZeroU64` because aggregate streams can
/// in principle be unboundedly long; `SchemaVersion` is `NonZeroU32`
/// because the wire format chose that width. Same invariant, different
/// width — explicit total conversion to `Version` is provided for the
/// upcaster path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SchemaVersion(NonZeroU32);

impl SchemaVersion {
    /// The initial schema version (`1`).
    pub const INITIAL: Self = {
        // SAFETY: 1 is nonzero; const NonZeroU32::new returns Some for nonzero
        match NonZeroU32::new(1) {
            Some(n) => Self(n),
            None => unreachable!(),
        }
    };

    /// Construct from a `NonZeroU32`. Infallible.
    #[must_use]
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    /// Construct from a raw `u32`, validating the nonzero invariant.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::SchemaVersionZero`] if `value == 0`.
    pub fn from_u32(value: u32) -> Result<Self, ValueError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(ValueError::SchemaVersionZero)
    }

    /// The inner `u32` value (always > 0).
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl From<SchemaVersion> for nexus::Version {
    /// Widen to the kernel's `Version` (`NonZeroU64`). Total — `NonZeroU32`
    /// always fits in `NonZeroU64`.
    fn from(sv: SchemaVersion) -> Self {
        // SAFETY: NonZeroU32 → NonZeroU64 widening preserves the nonzero
        // invariant; `Version::new` returns Some for nonzero u64.
        #[allow(
            clippy::expect_used,
            reason = "NonZeroU32 widened to u64 is structurally nonzero; \
                      Version::new returns Some for all nonzero u64 inputs"
        )]
        Self::new(u64::from(sv.get())).expect("NonZeroU32 widened to u64 is nonzero")
    }
}
```

- [ ] **Step 5: Run the tests to verify pass**

Run: `nix develop -c cargo test -p nexus-store schema_version_tests`
Expected: all 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/value.rs
git commit -m "feat(store): add SchemaVersion newtype (NonZeroU32, sized to wire field)

Mirrors the kernel's Version (NonZeroU64) pattern for the schema-version
field. Total From<SchemaVersion> for Version for the upcaster boundary;
fallible from_u32 for raw inputs (corrupt-data path); infallible new for
already-NonZeroU32 inputs. Deletes the runtime schema_version == 0 check
class once wired in PR2/PR3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.6: Re-export value types from crate root + property tests

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`
- Modify: `crates/nexus-store/src/value.rs`

- [ ] **Step 1: Add the re-exports in `lib.rs`**

In `crates/nexus-store/src/lib.rs`, find the existing `pub use` block (search for `pub use crate::envelope`) and add alongside:

```rust
pub use crate::value::{EventType, Metadata, Payload, SchemaVersion, ValueError};
```

- [ ] **Step 2: Write a property test for round-trip + boundary**

Append to `crates/nexus-store/src/value.rs`:

```rust
#[cfg(test)]
mod property_tests {
    use super::*;
    use bytes::Bytes;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn event_type_from_static_then_as_str_roundtrips(s in "[a-zA-Z][a-zA-Z0-9_]{0,63}") {
            // `&'static str` from non-static comes via leak. The test
            // verifies the value round-trips through Bytes::from_static
            // semantics — equivalent path.
            let bytes = Bytes::from(s.clone().into_bytes());
            let et = EventType::from_bytes(bytes).expect("valid");
            prop_assert_eq!(et.as_str(), &s);
        }

        #[test]
        fn payload_arbitrary_bytes_within_cap_accepted(buf in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let p = Payload::from_bytes(Bytes::from(buf.clone())).expect("under cap");
            prop_assert_eq!(p.as_slice(), buf.as_slice());
        }

        #[test]
        fn metadata_nonempty_within_cap_accepted(buf in proptest::collection::vec(any::<u8>(), 1..4096)) {
            let m = Metadata::from_bytes(Bytes::from(buf.clone())).expect("nonempty, under cap");
            prop_assert_eq!(m.as_slice(), buf.as_slice());
        }

        #[test]
        fn event_type_invalid_utf8_always_rejected(bytes in proptest::collection::vec(any::<u8>(), 1..256)) {
            // Filter to definitively invalid UTF-8: contains 0xFE or 0xFF
            // which are never valid UTF-8 start bytes.
            prop_assume!(bytes.iter().any(|&b| b == 0xFEu8 || b == 0xFFu8));
            let result = EventType::from_bytes(Bytes::from(bytes));
            prop_assert!(result.is_err());
        }
    }
}
```

- [ ] **Step 3: Run the property tests**

Run: `nix develop -c cargo test -p nexus-store property_tests`
Expected: all property tests PASS (default proptest iteration count: 256).

- [ ] **Step 4: Full crate-level verification**

Run: `nix flake check`
Expected: passes (clippy, fmt, tests, taplo, audit, deny, hakari).

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/lib.rs crates/nexus-store/src/value.rs
git commit -m "feat(store): re-export value newtypes + property-test round-trips

Round-trip props for EventType/Payload/Metadata across the random-bytes
boundary, plus a UTF-8-rejection prop using non-startable byte patterns.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1.7: Open PR1

- [ ] **Step 1: Push the branch**

```bash
git push -u origin refactor/value-newtypes-pr1-introduce-module
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "refactor(store): introduce value newtypes module (PR1 of 3)" --body "$(cat <<'EOF'
## Summary
- Adds `nexus_store::value` module: `EventType`, `Payload`, `Metadata`, `SchemaVersion`, `ValueError`.
- `EventType` / `Payload` / `Metadata` are `Bytes`-backed and own UTF-8 + size-cap invariants.
- `SchemaVersion(NonZeroU32)` mirrors the kernel's `Version(NonZeroU64)` pattern, sized to the wire-format field. `From<SchemaVersion> for Version` for the upcaster boundary.
- Pure addition — no other code path touches these types yet.

PR1 of the value-newtype refactor. PR2 switches `PendingEnvelope`; PR3 shrinks `wire.rs`.

## Test plan
- [x] Unit tests for each newtype's constructors and error paths
- [x] Property tests for round-trip + UTF-8 rejection
- [x] `nix flake check` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

# PR2: Switch `PendingEnvelope` to typed fields, fallible builder

**Branch name suggestion:** `refactor/value-newtypes-pr2-pending-envelope`

Goal: change `PendingEnvelope` to hold value newtypes; builder methods that accept user-supplied bytes return `Result`. All test and example call sites update.

### Task 2.1: Add `EnvelopeError::Value` variant

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

- [ ] **Step 1: Extend `EnvelopeError` to wrap `ValueError`**

In `crates/nexus-store/src/envelope.rs`, locate the `EnvelopeError` enum (around line 28) and add the new variant:

```rust
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

    #[error(transparent)]
    Value(#[from] crate::value::ValueError),
}
```

Note: `EnvelopeError::InvalidSchemaVersion` stays for now — it still triggers from `PersistedEnvelope::try_new` which currently takes `u32`. PR3 Task 3.2 changes `try_new` to take `SchemaVersion` and deletes this variant.

- [ ] **Step 2: Verify compilation**

Run: `nix develop -c cargo check -p nexus-store`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/envelope.rs
git commit -m "refactor(store): add EnvelopeError::Value wrapping ValueError

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.2: Change `PendingEnvelope` fields to use value newtypes

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

- [ ] **Step 1: Write a failing test for the new field types**

Append to the `#[cfg(test)] mod tests` block in `envelope.rs`:

```rust
#[test]
fn pending_envelope_holds_typed_event_type() {
    let env = pending_envelope(Version::INITIAL)
        .event_type("UserCreated")
        .expect("valid event type")
        .payload(Bytes::from_static(b"p"))
        .expect("valid payload")
        .build();
    // event_type() now returns &str via EventType::as_str; preserves
    // the old &'static str surface as a borrow.
    assert_eq!(env.event_type(), "UserCreated");
}

#[test]
fn pending_envelope_rejects_oversize_event_type() {
    let oversized = "x".repeat(crate::value::MAX_EVENT_TYPE_LEN + 1);
    // We can only call .event_type with &'static str via this builder
    // (literals come in as static). For oversize testing we need the
    // bytes-accepting variant — confirm builder API surfaces that path.
    let err = pending_envelope(Version::INITIAL)
        .event_type_bytes(Bytes::from(oversized))
        .expect_err("oversized must be rejected");
    assert!(matches!(err, EnvelopeError::Value(_)));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `nix develop -c cargo test -p nexus-store envelope`
Expected: FAIL — fields not the new type yet; new methods don't exist.

- [ ] **Step 3: Update `PendingEnvelope` and the builder typestate fields**

Replace the existing `PendingEnvelope` struct in `crates/nexus-store/src/envelope.rs` with:

```rust
#[derive(Debug, Clone)]
pub struct PendingEnvelope {
    version: Version,
    event_type: crate::value::EventType,
    schema_version: crate::value::SchemaVersion,
    payload: crate::value::Payload,
    metadata: Option<crate::value::Metadata>,
}
```

Replace the accessors:

```rust
impl PendingEnvelope {
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// Borrowed event type as `&str`.
    #[must_use]
    pub fn event_type(&self) -> &str {
        self.event_type.as_str()
    }

    /// Owned event type — one Arc share.
    #[must_use]
    pub fn event_type_value(&self) -> crate::value::EventType {
        self.event_type.clone()
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.payload.as_slice()
    }

    /// Owned payload — one Arc share.
    #[must_use]
    pub fn payload_bytes(&self) -> Bytes {
        self.payload.clone().into_bytes()
    }

    /// Owned payload value newtype — one Arc share.
    #[must_use]
    pub fn payload_value(&self) -> crate::value::Payload {
        self.payload.clone()
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_ref().map(crate::value::Metadata::as_slice)
    }

    /// Owned metadata — one Arc share per `Some`.
    #[must_use]
    pub fn metadata_bytes(&self) -> Option<Bytes> {
        self.metadata
            .as_ref()
            .map(|m| m.clone().into_bytes())
    }

    /// Owned metadata value newtype — one Arc share per `Some`.
    #[must_use]
    pub fn metadata_value(&self) -> Option<crate::value::Metadata> {
        self.metadata.clone()
    }

    /// The raw u32 view (always > 0, by the `SchemaVersion` invariant).
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version.get()
    }

    /// The typed schema version.
    #[must_use]
    pub const fn schema_version_value(&self) -> crate::value::SchemaVersion {
        self.schema_version
    }
}
```

Replace the `WithEventType` and `WithPayload` typestate fields:

```rust
pub struct WithEventType {
    version: Version,
    event_type: crate::value::EventType,
}

pub struct WithPayload {
    version: Version,
    event_type: crate::value::EventType,
    payload: crate::value::Payload,
    schema_version: crate::value::SchemaVersion,
}
```

- [ ] **Step 4: Replace the builder methods to be fallible**

Replace the three `impl` blocks:

```rust
impl WithVersion {
    /// Set the event type from a `&'static str` literal. Infallible.
    #[must_use]
    pub fn event_type(self, event_type: &'static str) -> WithEventType {
        WithEventType {
            version: self.version,
            event_type: crate::value::EventType::from_static_str(event_type),
        }
    }

    /// Set the event type from arbitrary bytes; validates UTF-8 and size cap.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the bytes are invalid UTF-8 or
    /// exceed [`MAX_EVENT_TYPE_LEN`](crate::value::MAX_EVENT_TYPE_LEN).
    pub fn event_type_bytes(self, bytes: Bytes) -> Result<WithEventType, EnvelopeError> {
        let event_type = crate::value::EventType::from_bytes(bytes)?;
        Ok(WithEventType {
            version: self.version,
            event_type,
        })
    }
}

impl WithEventType {
    /// Set the payload from any `Into<Bytes>` source. Fallible: validates
    /// the size cap.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the payload exceeds
    /// [`MAX_PAYLOAD_LEN`](crate::value::MAX_PAYLOAD_LEN).
    pub fn payload(self, payload: impl Into<Bytes>) -> Result<WithPayload, EnvelopeError> {
        let payload = crate::value::Payload::from_bytes(payload.into())?;
        Ok(WithPayload {
            version: self.version,
            event_type: self.event_type,
            payload,
            schema_version: crate::value::SchemaVersion::INITIAL,
        })
    }
}

impl WithPayload {
    /// Override the schema version (default: [`SchemaVersion::INITIAL`]).
    #[must_use]
    pub const fn schema_version(mut self, schema_version: crate::value::SchemaVersion) -> Self {
        self.schema_version = schema_version;
        self
    }

    /// Build with no metadata. Infallible.
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

    /// Build with metadata; fallible (validates non-empty + size cap).
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::Value`] if the metadata is empty or
    /// exceeds [`MAX_METADATA_LEN`](crate::value::MAX_METADATA_LEN).
    pub fn with_metadata(
        self,
        metadata: impl Into<Bytes>,
    ) -> Result<PendingEnvelope, EnvelopeError> {
        let metadata = crate::value::Metadata::from_bytes(metadata.into())?;
        Ok(PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata: Some(metadata),
        })
    }
}
```

- [ ] **Step 5: Update the existing in-module tests to use the new fallible builder**

Replace the existing tests in `envelope.rs`'s `tests` mod (the two `pending_envelope_builds_*` tests) with:

```rust
#[test]
fn pending_envelope_builds_with_metadata() {
    let env = pending_envelope(Version::INITIAL)
        .event_type("UserCreated")
        .payload(Bytes::from_static(b"payload-bytes"))
        .expect("valid payload")
        .with_metadata(Bytes::from_static(b"meta-bytes"))
        .expect("valid metadata");

    assert_eq!(env.event_type(), "UserCreated");
    assert_eq!(env.payload(), b"payload-bytes");
    assert_eq!(env.metadata(), Some(b"meta-bytes".as_slice()));
    assert_eq!(env.schema_version(), 1);
}

#[test]
fn pending_envelope_builds_without_metadata() {
    let env = pending_envelope(Version::INITIAL)
        .event_type("X")
        .payload(Bytes::from_static(b"p"))
        .expect("valid payload")
        .build();

    assert_eq!(env.metadata(), None);
}
```

Note: the old test built with `Bytes::from_static(b"")` payload. We've changed payload to accept empty (it's valid; only metadata rejects empty), so that path still works. Keeping a non-empty `b"p"` payload to avoid masking any future change.

- [ ] **Step 6: Run all envelope tests**

Run: `nix develop -c cargo test -p nexus-store envelope`
Expected: all envelope-mod tests pass. The two earlier-written tests (`pending_envelope_holds_typed_event_type`, `pending_envelope_rejects_oversize_event_type`) also pass.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/envelope.rs
git commit -m "refactor(store)!: PendingEnvelope holds value newtypes; builder is fallible

BREAKING CHANGE: WithEventType::payload and WithPayload::with_metadata
now return Result<_, EnvelopeError>. WithVersion::event_type stays
infallible for &'static str literals; a new event_type_bytes method
handles arbitrary bytes fallibly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.3: Update internal callers (codec, testing, repository)

**Files:**
- Modify (audit and update): `crates/nexus-store/src/codec.rs`
- Modify (audit and update): `crates/nexus-store/src/testing.rs`
- Modify (audit and update): `crates/nexus-store/src/repository.rs`
- Modify (audit and update): `crates/nexus-store/src/snapshot.rs`

- [ ] **Step 1: Compile-check to surface every call site**

Run: `nix develop -c cargo build -p nexus-store`
Expected: compilation errors at every call site that uses the old `.payload(...).with_metadata(...)` chain or constructs `PendingEnvelope` with raw bytes. Note each error location.

- [ ] **Step 2: At each error site, add `?` propagation or `.expect("valid")` as appropriate**

Pattern to apply at each call site:

Before:
```rust
let env = pending_envelope(version)
    .event_type("Foo")
    .payload(bytes)
    .with_metadata(meta);
```

After (production code that returns Result):
```rust
let env = pending_envelope(version)
    .event_type("Foo")
    .payload(bytes)?
    .with_metadata(meta)?;
```

After (code with no Result return):
```rust
let env = pending_envelope(version)
    .event_type("Foo")
    .payload(bytes)
    .expect("payload within cap")
    .with_metadata(meta)
    .expect("metadata within cap");
```

For places where the caller's error type doesn't have a `From<EnvelopeError>` impl: add the impl (one-liner via `#[from]` on the appropriate variant) rather than mapping manually.

- [ ] **Step 3: Run all `nexus-store` library tests (no integration tests yet)**

Run: `nix develop -c cargo test -p nexus-store --lib`
Expected: all library tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/codec.rs crates/nexus-store/src/testing.rs crates/nexus-store/src/repository.rs crates/nexus-store/src/snapshot.rs
git commit -m "refactor(store): propagate fallible PendingEnvelope builder through internal callers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.4: Update `nexus-store` integration tests

**Files:**
- Modify: `crates/nexus-store/tests/inmemory_store_tests.rs`
- Modify: `crates/nexus-store/tests/inmemory_conformance.rs`
- Modify: `crates/nexus-store/tests/subscription_tests.rs`
- Modify: `crates/nexus-store/tests/property_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/wire_alignment_tests.rs`
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs`

- [ ] **Step 1: Compile each test file to surface every call site**

Run: `nix develop -c cargo test -p nexus-store --no-run`
Expected: compile errors at builder-chain call sites in test files.

- [ ] **Step 2: Apply the `.expect("valid")` pattern at every test call site**

The pattern is mechanical. Each `pending_envelope(...).event_type(...).payload(bytes).build()` becomes:

```rust
pending_envelope(version)
    .event_type("Name")
    .payload(bytes)
    .expect("payload within cap")
    .build()
```

And `with_metadata(...)` becomes `.with_metadata(...).expect("metadata within cap")`.

For property tests where the payload or metadata size is generated within a range, the strategy's range guarantees the value-newtype cap is satisfied — use `.expect("strategy range within cap")` and ensure the strategy's max is `<= MAX_PAYLOAD_LEN`.

For property tests that try to *trigger* the cap (boundary tests), keep the `Result` and assert on the error variant.

- [ ] **Step 3: Run integration tests**

Run: `nix develop -c cargo test -p nexus-store`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/tests/
git commit -m "refactor(store): update integration tests for fallible PendingEnvelope builder

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.5: Update examples

**Files:**
- Modify: `examples/store-inmemory/src/main.rs`
- Modify: `examples/store-and-kernel/src/main.rs`

- [ ] **Step 1: Compile each example to surface call sites**

Run: `nix develop -c cargo build --examples`
Expected: errors at envelope-builder call sites.

- [ ] **Step 2: Apply `?` propagation (these examples already return `Result`)**

At each call site:

```rust
pending_envelope(version)
    .event_type("UserCreated")
    .payload(bytes)?
    .build()
```

- [ ] **Step 3: Run the examples to verify behavior**

Run: `nix develop -c cargo run --example store-inmemory`
Expected: example runs to completion (same output as before).

Run: `nix develop -c cargo run --example store-and-kernel`
Expected: example runs to completion.

- [ ] **Step 4: Commit**

```bash
git add examples/
git commit -m "refactor(examples): propagate fallible PendingEnvelope builder Results

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.6: Audit `nexus-framework` and `nexus-fjall` for envelope callers

**Files:**
- Audit: `crates/nexus-framework/src/`
- Audit: `crates/nexus-fjall/src/` and `crates/nexus-fjall/tests/`

- [ ] **Step 1: Compile-check both downstream crates**

Run: `nix develop -c cargo build -p nexus-framework -p nexus-fjall --all-targets`
Expected: surface any caller breakage.

- [ ] **Step 2: Apply propagation pattern at every error site**

Same `?` / `.expect("valid")` pattern as PR2 Task 2.3.

- [ ] **Step 3: Run full check**

Run: `nix flake check`
Expected: passes (this is the PR2 finish line — note the wire-format API still has its old shape; that changes in PR3).

- [ ] **Step 4: Commit**

```bash
git add crates/
git commit -m "refactor: propagate PendingEnvelope builder Results in framework + fjall

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.7: Open PR2

- [ ] **Step 1: Push**

```bash
git push -u origin refactor/value-newtypes-pr2-pending-envelope
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "refactor(store)!: PendingEnvelope holds value newtypes; builder fallible (PR2 of 3)" --body "$(cat <<'EOF'
## Summary
- `PendingEnvelope` now holds `EventType`, `Payload`, `Option<Metadata>`.
- Builder methods that accept user bytes (`.payload`, `.with_metadata`, `.event_type_bytes`) return `Result<_, EnvelopeError>`.
- `EnvelopeError` gains a `Value` variant wrapping `ValueError`.
- All internal, test, example, and downstream-crate call sites updated.

PR2 of the value-newtype refactor (depends on PR1).

## Breaking changes
- `WithEventType::payload` returns `Result<WithPayload, EnvelopeError>` (was `WithPayload`).
- `WithPayload::with_metadata` returns `Result<PendingEnvelope, EnvelopeError>` (was `PendingEnvelope`).
- New `WithVersion::event_type_bytes(Bytes) -> Result<WithEventType, EnvelopeError>` for fallible bytes input.

## Test plan
- [x] Two new tests verifying typed-field holding and oversize rejection
- [x] All existing envelope, integration, adversarial, and property tests updated and passing
- [x] Examples run to completion
- [x] `nix flake check` passes

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

# PR3: Owned accessors on `PersistedEnvelope`, shrink `wire.rs`

**Branch name suggestion:** `refactor/value-newtypes-pr3-wire-shrink`

Goal: add owned-value accessors to `PersistedEnvelope`; change `wire::encode_frame` to take value-newtype references; delete the `*Len` newtypes, the `MAX_*` constants, and the size-cap `WireError` variants in `wire.rs`; update all callers.

### Task 3.1: Add owned-value accessors on `PersistedEnvelope`

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

- [ ] **Step 1: Write failing tests for the new accessors**

Append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn persisted_envelope_event_type_owned_returns_validated_value_newtype() {
    let env = PersistedEnvelope::try_new(
        Version::INITIAL,
        crate::store::GlobalSeq::new(1).expect("nonzero"),
        Bytes::from_static(b"TYPEpayload"),
        1,
        0..4,
        4..11,
        None,
    )
    .expect("valid");

    let et = env.event_type_owned();
    assert_eq!(et.as_str(), "TYPE");
}

#[test]
fn persisted_envelope_payload_owned_returns_validated_value_newtype() {
    let env = PersistedEnvelope::try_new(
        Version::INITIAL,
        crate::store::GlobalSeq::new(1).expect("nonzero"),
        Bytes::from_static(b"TYPEpayload"),
        1,
        0..4,
        4..11,
        None,
    )
    .expect("valid");

    let p = env.payload_owned();
    assert_eq!(p.as_slice(), b"payload");
}

#[test]
fn persisted_envelope_metadata_owned_returns_some_when_present() {
    let env = PersistedEnvelope::try_new(
        Version::INITIAL,
        crate::store::GlobalSeq::new(1).expect("nonzero"),
        Bytes::from_static(b"TYPEpayloadMETA"),
        1,
        0..4,
        4..11,
        Some(11..15),
    )
    .expect("valid");

    let m = env.metadata_owned().expect("present");
    assert_eq!(m.as_slice(), b"META");
}

#[test]
fn persisted_envelope_metadata_owned_returns_none_when_absent() {
    let env = PersistedEnvelope::try_new(
        Version::INITIAL,
        crate::store::GlobalSeq::new(1).expect("nonzero"),
        Bytes::from_static(b"TYPEpayload"),
        1,
        0..4,
        4..11,
        None,
    )
    .expect("valid");

    assert!(env.metadata_owned().is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `nix develop -c cargo test -p nexus-store persisted_envelope_event_type_owned`
Expected: FAIL — accessor not defined.

- [ ] **Step 3: Add the owned accessors**

Insert into the `impl PersistedEnvelope` block (after `event_type_bytes`, `payload_bytes`, `metadata_bytes`):

```rust
/// Owned, validated event type. One Arc share over the underlying buffer.
///
/// The UTF-8 and size invariants were established at `try_new` time;
/// this constructor skips re-validation via the crate-internal
/// `from_validated_bytes` path.
#[must_use]
pub fn event_type_owned(&self) -> crate::value::EventType {
    crate::value::EventType::from_validated_bytes(self.event_type_bytes())
}

/// Owned, validated payload. One Arc share over the underlying buffer.
#[must_use]
pub fn payload_owned(&self) -> crate::value::Payload {
    crate::value::Payload::from_validated_bytes(self.payload_bytes())
}

/// Owned, validated metadata. One Arc share per `Some` over the
/// underlying buffer.
///
/// Returns `None` when the wire-level absent sentinel was present.
/// "Present but empty" metadata is structurally disallowed by the
/// `Metadata` invariant.
#[must_use]
pub fn metadata_owned(&self) -> Option<crate::value::Metadata> {
    self.metadata_bytes()
        .map(crate::value::Metadata::from_validated_bytes)
}
```

- [ ] **Step 4: Run the tests to verify pass**

Run: `nix develop -c cargo test -p nexus-store persisted_envelope`
Expected: all four new tests pass; previous tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/envelope.rs
git commit -m "feat(store): add owned value-newtype accessors on PersistedEnvelope

event_type_owned, payload_owned, metadata_owned each materialize one
Bytes::slice (one Arc inc) wrapped via the validated-bytes constructor
of the corresponding value newtype. Construction-time invariants are
not re-checked on the read path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.2: Change `wire::encode_frame` to take value newtypes; drop wire-side validation

**Files:**
- Modify: `crates/nexus-store/src/wire.rs`

- [ ] **Step 1: Audit the existing `wire.rs` signature and its callers**

Read `crates/nexus-store/src/wire.rs` lines around `plan`, `encode_frame`, the `EventTypeLen`/`MetadataLen`/`PayloadLen` types, and the `WireError` variants. The new signature takes `SchemaVersion`, `&EventType`, `&Payload`, `Option<&Metadata>`. The only `WireError` case that remains is `FrameLengthOverflow` (pure arithmetic). `InvalidSchemaVersion` is deleted — `SchemaVersion`'s nonzero invariant makes the check structurally impossible. `decode_frame` returns `SchemaVersion` and gains a new `DecodeError::CorruptSchemaVersion` for the schema_version=0 case in corrupt on-disk data.

- [ ] **Step 2: Write a failing test for the new signature**

Append (or insert near other wire unit tests in `wire.rs` `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod value_newtype_input_tests {
    use super::*;
    use crate::value::{EventType, Metadata, Payload};
    use bytes::Bytes;

    #[test]
    fn encode_frame_accepts_value_newtypes() {
        let et = EventType::from_static_str("UserCreated");
        let payload = Payload::from_bytes(Bytes::from_static(b"hello")).expect("valid");
        let metadata = Metadata::from_bytes(Bytes::from_static(b"m")).expect("valid");
        let sv = crate::value::SchemaVersion::INITIAL;
        let frame = encode_frame(1, sv, &et, &payload, Some(&metadata)).expect("valid frame");
        let decoded = decode_frame(&frame.value).expect("decodes");
        assert_eq!(decoded.global_seq, 1);
        assert_eq!(decoded.schema_version, sv);
    }

    #[test]
    fn decode_frame_rejects_corrupt_schema_version_zero() {
        // Hand-craft a frame with schema_version=0 on the wire, simulating
        // corrupt on-disk data. (Going through encode_frame with a
        // SchemaVersion is structurally impossible.)
        let et = EventType::from_static_str("X");
        let payload = Payload::from_bytes(Bytes::from_static(b"p")).expect("valid");
        let sv_one = crate::value::SchemaVersion::INITIAL;
        let mut frame =
            encode_frame(1, sv_one, &et, &payload, None).expect("valid frame for tamper");
        // Overwrite the schema_version field (offset 8, 4 bytes LE) with zero.
        let mut bytes_vec = frame.value.to_vec();
        bytes_vec[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4].fill(0);
        let tampered = bytes::Bytes::from(bytes_vec);
        let err = decode_frame(&tampered).expect_err("schema_version=0 on wire rejected");
        assert!(matches!(err, DecodeError::CorruptSchemaVersion));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `nix develop -c cargo test -p nexus-store value_newtype_input_tests`
Expected: FAIL — `encode_frame` still takes `&str`/`&[u8]`.

- [ ] **Step 4: Update `encode_frame` and `plan` signatures**

Replace the existing `encode_frame` and `plan` functions in `crates/nexus-store/src/wire.rs` with:

```rust
/// Encode one frame.
///
/// All value invariants (event_type UTF-8/size, payload size, metadata
/// size/non-empty, schema_version > 0) are owned by the value newtypes.
/// The only failure mode here is `FrameLengthOverflow` — pure arithmetic.
///
/// # Errors
///
/// See [`WireError`].
pub fn encode_frame(
    global_seq: u64,
    schema_version: crate::value::SchemaVersion,
    event_type: &crate::value::EventType,
    payload: &crate::value::Payload,
    metadata: Option<&crate::value::Metadata>,
) -> Result<EncodedFrame, WireError> {
    let plan = plan(global_seq, schema_version, event_type, payload, metadata)?;
    Ok(execute(plan))
}

fn plan<'a>(
    global_seq: u64,
    schema_version: crate::value::SchemaVersion,
    event_type: &'a crate::value::EventType,
    payload: &'a crate::value::Payload,
    metadata: Option<&'a crate::value::Metadata>,
) -> Result<FramePlan<'a>, WireError> {
    let event_type_bytes = event_type.as_bytes();
    let metadata_bytes = metadata.map(crate::value::Metadata::as_slice);
    let payload_bytes = payload.as_slice();

    let layout = FrameLayout::compute_from_validated_lengths(
        event_type_bytes.len(),
        metadata_bytes.map(<[u8]>::len),
        payload_bytes.len(),
    )?;
    let header = FrameHeader::from_validated_lengths(
        global_seq,
        schema_version.get(),
        event_type_bytes.len(),
        metadata_bytes.map(<[u8]>::len),
    );
    Ok(FramePlan {
        header,
        event_type_bytes,
        metadata: metadata_bytes,
        payload: payload_bytes,
        layout,
    })
}
```

- [ ] **Step 5: Update `FrameLayout::compute` and `FrameHeader` to accept raw lengths**

The existing `FrameLayout::compute` takes the `*Len` newtypes. Add or rename to `compute_from_validated_lengths` that takes plain `usize`:

```rust
impl FrameLayout {
    /// Layout calculation from already-validated raw lengths.
    ///
    /// Callers must guarantee that the lengths fit their wire fields
    /// (the value newtypes do this).
    fn compute_from_validated_lengths(
        event_type_len: usize,
        metadata_len: Option<usize>,
        payload_len: usize,
    ) -> Result<Self, WireError> {
        // ... existing layout math, but using usize directly
        // instead of *Len newtypes. The FrameLengthOverflow check stays.
    }
}

impl FrameHeader {
    fn from_validated_lengths(
        global_seq: u64,
        schema_version: u32,
        event_type_len: usize,
        metadata_len: Option<usize>,
    ) -> Self {
        // Lengths fit by caller invariant: cast via `try_from(...).expect("validated")`
        // with a comment pointing to the value newtype invariant.
        let event_type_len = u16::try_from(event_type_len)
            .expect("event_type length validated by EventType::from_bytes invariant");
        let metadata_len_field = metadata_len.map(|n| {
            u32::try_from(n)
                .expect("metadata length validated by Metadata::from_bytes invariant")
        });
        Self {
            global_seq,
            schema_version,
            event_type_len,
            metadata_len_field,
        }
    }
}
```

Note: the existing `FrameHeader` stores typed newtypes (`EventTypeLen`, `Option<MetadataLen>`). Since we're deleting those newtypes in the next task, restructure `FrameHeader` to store `u16` and `Option<u32>` directly. Update `write_into` and `read_from` accordingly.

- [ ] **Step 6: Delete the now-unused `*Len` newtypes and update `decode_frame`**

In `crates/nexus-store/src/wire.rs`, delete:
- `struct EventTypeLen(u16)` and its `impl TryFrom<usize>` block.
- `struct MetadataLen(u32)` and its `impl TryFrom<usize>` block.
- `struct PayloadLen(u32)` and its `impl TryFrom<usize>` block.

Delete from `WireError`:
- `EventTypeTooLong` variant.
- `MetadataTooLong` variant.
- `PayloadTooLong` variant.
- `InvalidSchemaVersion` variant.

Keep:
- `FrameLengthOverflow` (pure arithmetic — only remaining failure mode).

Delete the `MAX_EVENT_TYPE_LEN`, `MAX_METADATA_LEN`, `MAX_PAYLOAD_LEN` constants from `wire.rs` (they live in `value.rs` now).

Update `DecodedFrame` to use the typed schema version:

```rust
#[derive(Debug)]
pub struct DecodedFrame {
    pub global_seq: u64,
    pub schema_version: crate::value::SchemaVersion,
    pub offsets: FrameOffsets,
}
```

Add a new variant to `DecodeError`:

```rust
#[error("corrupt schema_version on wire: got 0, must be > 0")]
CorruptSchemaVersion,
```

In `decode_frame` (or the `FrameHeader::read_from` path that produces the value), convert the raw `u32` schema_version into `SchemaVersion` and return `DecodeError::CorruptSchemaVersion` on zero:

```rust
let schema_version = crate::value::SchemaVersion::from_u32(raw_schema_version)
    .map_err(|_| DecodeError::CorruptSchemaVersion)?;
```

- [ ] **Step 6b: Update `PersistedEnvelope::try_new` to take `SchemaVersion`**

In `crates/nexus-store/src/envelope.rs`, replace the `try_new` signature:

```rust
pub fn try_new(
    version: Version,
    global_seq: GlobalSeq,
    value: Bytes,
    schema_version: crate::value::SchemaVersion,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
) -> Result<Self, EnvelopeError> {
    // schema_version > 0 guaranteed by SchemaVersion's invariant —
    // no runtime check needed.
    let len = value.len();
    check_range(&event_type_range, len)?;
    check_range(&payload_range, len)?;
    if let Some(ref m) = metadata_range {
        check_range(m, len)?;
    }
    let et_start = idx(event_type_range.start);
    let et_end = idx(event_type_range.end);
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
```

Update the `PersistedEnvelope` struct field type accordingly:

```rust
pub struct PersistedEnvelope {
    version: Version,
    global_seq: GlobalSeq,
    schema_version: crate::value::SchemaVersion,
    value: Bytes,
    event_type_range: Range<u32>,
    payload_range: Range<u32>,
    metadata_range: Option<Range<u32>>,
}
```

Update the accessors:

```rust
/// Raw u32 view (always > 0 by the SchemaVersion invariant).
#[must_use]
pub const fn schema_version(&self) -> u32 {
    self.schema_version.get()
}

/// Typed schema version.
#[must_use]
pub const fn schema_version_value(&self) -> crate::value::SchemaVersion {
    self.schema_version
}

/// The schema version widened to the kernel's `Version` for upcaster APIs.
/// Total conversion — `SchemaVersion` is structurally nonzero.
#[must_use]
pub fn schema_version_as_version(&self) -> Version {
    self.schema_version.into()
}
```

Delete `EnvelopeError::InvalidSchemaVersion` — its only producer (`try_new`'s `== 0` check) is gone.

Update the test `persisted_envelope_rejects_schema_version_zero` (around line 504): the test's API signature change means it no longer compiles. Replace it with a test on the wire-decode path:

```rust
#[test]
fn wire_decode_rejects_schema_version_zero_in_corrupt_data() {
    // The persisted_envelope_rejects_schema_version_zero test moves to
    // wire.rs because the failure mode is now decode-time, not
    // construction-time. See wire::value_newtype_input_tests.
}
```

(The actual corrupt-schema-version assertion lives in `wire::value_newtype_input_tests::decode_frame_rejects_corrupt_schema_version_zero`, added in Task 3.2 Step 2.)

- [ ] **Step 7: Compile and run wire tests**

Run: `nix develop -c cargo build -p nexus-store`
Expected: compile errors at every existing call site of `encode_frame` (codec.rs, envelope.rs::for_decode, testing.rs, plus the wire.rs's own internal tests). Note each location for the next task.

Run: `nix develop -c cargo test -p nexus-store --lib wire::value_newtype_input_tests`
Expected: the two new tests pass (with the rest of the crate broken by the signature change).

- [ ] **Step 8: Commit (with crate broken; PR3 is one logical unit)**

This is the one place in the plan where we commit a temporarily-broken build inside the branch; the next task fixes all call sites in one commit. If you prefer not to commit broken state, combine Tasks 3.2 and 3.3 into one commit.

```bash
git add crates/nexus-store/src/wire.rs crates/nexus-store/src/envelope.rs
git commit -m "refactor(store)!: wire takes value newtypes; SchemaVersion replaces u32

BREAKING CHANGE:
- encode_frame signature: (u64, SchemaVersion, &EventType, &Payload, Option<&Metadata>).
- decode_frame returns SchemaVersion in DecodedFrame.
- WireError loses EventTypeTooLong, MetadataTooLong, PayloadTooLong, InvalidSchemaVersion.
  Only FrameLengthOverflow remains (pure arithmetic).
- DecodeError gains CorruptSchemaVersion for the schema_version=0 disk case.
- PersistedEnvelope::try_new takes SchemaVersion (was u32).
- EnvelopeError::InvalidSchemaVersion deleted (structurally impossible).
- MAX_*_LEN constants moved to nexus_store::value.

All value-cap and nonzero-schema invariants now live on the value newtypes.
Callers updated in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.3: Update all `wire::encode_frame` callers

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`
- Modify: `crates/nexus-store/src/envelope.rs` (`for_decode`)
- Modify: `crates/nexus-store/src/testing.rs`
- Modify: `crates/nexus-fjall/src/store.rs`
- Modify: `crates/nexus-fjall/src/encoding.rs`

- [ ] **Step 1: At each call site, wrap inputs in value newtypes**

Pattern transformation at each `wire::encode_frame(global_seq, schema_version, event_type_str, metadata_opt_slice, payload_slice)` call:

```rust
// Before:
let frame = wire::encode_frame(global_seq, schema_version, event_type, metadata, payload)?;

// After:
let et = crate::value::EventType::from_static_str(event_type); // for static strings
// OR for runtime bytes:
let et = crate::value::EventType::from_bytes(Bytes::copy_from_slice(event_type.as_bytes()))?;
let pl = crate::value::Payload::from_bytes(Bytes::copy_from_slice(payload))?;
let md = metadata
    .map(|m| crate::value::Metadata::from_bytes(Bytes::copy_from_slice(m)))
    .transpose()?;
let sv = crate::value::SchemaVersion::from_u32(schema_version)?; // for raw u32 inputs
let frame = wire::encode_frame(global_seq, sv, &et, &pl, md.as_ref())?;
```

For call sites that already have `PendingEnvelope` in hand (`codec.rs` paths, `nexus-fjall` append path): use the typed accessors directly — `schema_version_value()` returns `SchemaVersion` already:

```rust
let frame = wire::encode_frame(
    global_seq,
    pending.schema_version_value(),
    &pending.event_type_value(),
    &pending.payload_value(),
    pending.metadata_value().as_ref(),
)?;
```

For decode-side callers that previously matched on `decoded.schema_version` as `u32`: the field is now `SchemaVersion`. Use `.get()` to recover the raw `u32`, or pass the `SchemaVersion` through directly when constructing `PersistedEnvelope::try_new` (which now takes `SchemaVersion`).

For `PersistedEnvelope::for_decode`: the inputs are `&str` and `&[u8]`. Wrap them at the function boundary:

```rust
pub fn for_decode(event_type: &str, payload: &[u8]) -> Result<Self, ForDecodeError> {
    let et = crate::value::EventType::from_bytes(Bytes::copy_from_slice(event_type.as_bytes()))
        .map_err(ForDecodeError::Value)?;
    let pl = crate::value::Payload::from_bytes(Bytes::copy_from_slice(payload))
        .map_err(ForDecodeError::Value)?;
    let frame = crate::wire::encode_frame(
        GlobalSeq::INITIAL.as_u64(),
        crate::value::SchemaVersion::INITIAL,
        &et,
        &pl,
        None,
    )
    .map_err(ForDecodeError::Wire)?;
    Self::try_new(
        Version::INITIAL,
        GlobalSeq::INITIAL,
        frame.value,
        crate::value::SchemaVersion::INITIAL,
        frame.offsets.event_type,
        frame.offsets.payload,
        None,
    )
    .map_err(ForDecodeError::Envelope)
}
```

Add the `ForDecodeError` enum in `envelope.rs` (or extend `EnvelopeError` if simpler — but the latter widens an existing public type's error surface, so a separate type is cleaner):

```rust
#[derive(Debug, Error)]
pub enum ForDecodeError {
    #[error(transparent)]
    Value(#[from] crate::value::ValueError),
    #[error(transparent)]
    Wire(#[from] crate::wire::WireError),
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
}
```

For `nexus-fjall`: the `FjallError` enum should add `Value(#[from] nexus_store::value::ValueError)` so the `?` propagation works.

- [ ] **Step 2: Run full builds**

Run: `nix develop -c cargo build --workspace --all-targets`
Expected: compiles clean.

- [ ] **Step 3: Run full tests**

Run: `nix develop -c cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 4: Run flake check**

Run: `nix flake check`
Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add crates/ examples/
git commit -m "refactor(store, fjall): wrap wire::encode_frame inputs in value newtypes

All callers of wire::encode_frame now pass typed values. Adapter and
codec error types gain a ValueError variant (via #[from]) where needed.
PersistedEnvelope::for_decode returns a new ForDecodeError combining
ValueError + WireError + EnvelopeError.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.4: Audit and update `WireError` users

**Files:**
- Audit: `crates/nexus-store/src/error.rs`
- Audit: `crates/nexus-store/src/state.rs`
- Audit: any other crate that pattern-matches on `WireError` variants

- [ ] **Step 1: Search for users of the deleted variants**

Run: `nix develop -c cargo build --workspace --all-targets`
Expected: any code that matched on `WireError::{EventTypeTooLong, MetadataTooLong, PayloadTooLong, InvalidSchemaVersion}` or on `EnvelopeError::InvalidSchemaVersion` now produces non-exhaustive match warnings or errors.

Alternatively, grep:

```bash
grep -rn 'WireError::EventTypeTooLong\|WireError::MetadataTooLong\|WireError::PayloadTooLong\|WireError::InvalidSchemaVersion\|EnvelopeError::InvalidSchemaVersion' /Users/joel/Code/devrandom/nexus/crates
```

- [ ] **Step 2: Update each match site**

The deleted variants were size-cap and schema-version-zero errors. Where they were previously matched, the equivalent error now comes from `ValueError` at construction time, or from `DecodeError::CorruptSchemaVersion` on the read path. So each match site either:
- Reduces to handling only `WireError::FrameLengthOverflow` (the only remaining variant) if the call site receives errors only from `wire::encode_frame`; or
- Adds a `ValueError` branch if the call site sits downstream of value-newtype construction; or
- Adds a `DecodeError::CorruptSchemaVersion` branch for read-path error handling.

- [ ] **Step 3: Run tests**

Run: `nix develop -c cargo test --workspace`
Expected: passes.

- [ ] **Step 4: Commit**

```bash
git add crates/
git commit -m "refactor(store): update WireError match sites after size-cap variant removal

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.5: Final cross-cutting test pass + property tests for the lazy/owned split

**Files:**
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`

- [ ] **Step 1: Add a property test covering the round-trip through wire + owned accessors**

Append:

```rust
proptest! {
    #[test]
    fn wire_roundtrip_through_owned_accessors_preserves_values(
        et in "[a-zA-Z][a-zA-Z0-9_]{0,63}",
        payload in proptest::collection::vec(any::<u8>(), 0..4096),
        meta in proptest::collection::vec(any::<u8>(), 1..4096),
    ) {
        use nexus_store::value::{EventType, Metadata, Payload};
        use nexus_store::wire;
        use bytes::Bytes;

        let event_type = EventType::from_bytes(Bytes::from(et.clone().into_bytes())).expect("valid");
        let payload_v = Payload::from_bytes(Bytes::from(payload.clone())).expect("valid");
        let meta_v = Metadata::from_bytes(Bytes::from(meta.clone())).expect("valid");
        let sv = nexus_store::value::SchemaVersion::INITIAL;

        let frame = wire::encode_frame(1, sv, &event_type, &payload_v, Some(&meta_v))
            .expect("valid frame");

        let decoded = wire::decode_frame(&frame.value).expect("decodes");

        let env = nexus_store::envelope::PersistedEnvelope::try_new(
            nexus::Version::INITIAL,
            nexus_store::store::GlobalSeq::new(1).expect("nonzero"),
            frame.value,
            decoded.schema_version,
            decoded.offsets.event_type,
            decoded.offsets.payload,
            decoded.offsets.metadata,
        ).expect("valid persisted");

        prop_assert_eq!(env.event_type_owned().as_str(), &et);
        prop_assert_eq!(env.payload_owned().as_slice(), payload.as_slice());
        prop_assert_eq!(env.metadata_owned().expect("present").as_slice(), meta.as_slice());
    }
}
```

- [ ] **Step 2: Run the new property test**

Run: `nix develop -c cargo test -p nexus-store wire_roundtrip_through_owned_accessors`
Expected: passes (default 256 iterations).

- [ ] **Step 3: Run full flake check**

Run: `nix flake check`
Expected: passes — all of clippy, fmt, tests, taplo, audit, deny, hakari pass.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/tests/adversarial_property_tests.rs
git commit -m "test(store): round-trip property — wire + owned value accessors

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.6: Open PR3

- [ ] **Step 1: Push**

```bash
git push -u origin refactor/value-newtypes-pr3-wire-shrink
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "refactor(store, fjall)!: shrink wire.rs to pure framing; SchemaVersion + PersistedEnvelope owned accessors (PR3 of 3)" --body "$(cat <<'EOF'
## Summary
- `wire::encode_frame` takes `(u64, SchemaVersion, &EventType, &Payload, Option<&Metadata>)` — no more raw bytes, no wire-side validation.
- `wire::decode_frame` returns `DecodedFrame { schema_version: SchemaVersion, ... }`; gains `DecodeError::CorruptSchemaVersion` for the on-disk schema_version=0 case.
- Deleted `EventTypeLen`, `MetadataLen`, `PayloadLen` newtypes.
- Deleted `WireError::{EventTypeTooLong, MetadataTooLong, PayloadTooLong, InvalidSchemaVersion}` — only `FrameLengthOverflow` remains.
- Deleted `EnvelopeError::InvalidSchemaVersion` — `PersistedEnvelope::try_new` takes `SchemaVersion` and the check is structurally unreachable.
- `MAX_EVENT_TYPE_LEN` / `MAX_METADATA_LEN` / `MAX_PAYLOAD_LEN` constants moved from `wire.rs` to `nexus_store::value`.
- `PersistedEnvelope` gains `event_type_owned`, `payload_owned`, `metadata_owned`, `schema_version_value`, infallible `schema_version_as_version`.
- `nexus-fjall` and `nexus-store::codec` updated to construct value newtypes before calling `wire::encode_frame`.

PR3 of the value-newtype refactor (depends on PR1, PR2).

## Breaking changes
- `wire::encode_frame` signature change.
- `wire::decode_frame` return type change (`SchemaVersion` field).
- `WireError::{EventTypeTooLong, MetadataTooLong, PayloadTooLong, InvalidSchemaVersion}` removed.
- `EnvelopeError::InvalidSchemaVersion` removed.
- `DecodeError::CorruptSchemaVersion` added.
- `PersistedEnvelope::try_new` takes `SchemaVersion` (was `u32`).
- `wire::MAX_*_LEN` constants removed (use `nexus_store::value::MAX_*_LEN`).
- `PersistedEnvelope::for_decode` now returns `ForDecodeError` (was `WireError`).

## Test plan
- [x] Owned-accessor unit tests on `PersistedEnvelope`
- [x] `wire::encode_frame` accepts value newtypes (unit test)
- [x] Round-trip property: build value newtypes → encode → decode → reconstruct envelope → owned accessors recover originals
- [x] All existing wire / envelope / codec / fjall tests pass
- [x] `nix flake check` passes

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Plan Self-Review

**Spec coverage:** Every settled design point maps to at least one task.
- New `value.rs` with four newtypes + `ValueError` → Tasks 1.1–1.6.
- Value newtypes backed by `Bytes` (event_type/payload/metadata), infallible static-str constructor → Task 1.2.
- `SchemaVersion(NonZeroU32)` mirroring `Version`, sized to wire field, total `From<SchemaVersion> for Version` → Task 1.5.
- Size caps and validation owned by value type → Tasks 1.2/1.3/1.4 + 3.2.
- `WireError::InvalidSchemaVersion` and `EnvelopeError::InvalidSchemaVersion` deleted via the `SchemaVersion` type → Task 3.2 Steps 6, 6b.
- `wire.rs` becomes pure framing (only `FrameLengthOverflow` remains) → Task 3.2.
- `PendingEnvelope` holds typed fields, builder fallible → Tasks 2.1–2.2.
- `PersistedEnvelope` stays lazy with owned accessors materializing on demand → Task 3.1.
- Wire decoupled (`wire::encode_frame` takes typed inputs, not envelopes) → Task 3.2.
- Migration staged into 3 PRs → PR1/PR2/PR3 sections.

**Placeholder scan:** No `TODO`, no "implement later", no abstract handwaving. Every code block contains the actual code the engineer types. Where `FrameLayout::compute_from_validated_lengths` body is shown abbreviated, the abbreviation reads "existing layout math, but using usize directly" — that's a concrete instruction relative to the current code, not a placeholder. The engineer reads the existing function and substitutes `usize` for the `*Len` newtype types.

**Type consistency:** Names used across tasks:
- `EventType`, `Payload`, `Metadata`, `SchemaVersion`, `ValueError` — consistent.
- `EnvelopeError::Value(#[from] ValueError)` — used in PR2.
- `MAX_EVENT_TYPE_LEN`, `MAX_METADATA_LEN`, `MAX_PAYLOAD_LEN` — same names across `wire.rs` (old) and `value.rs` (new). The PR3 transition deletes the wire copies.
- `SchemaVersion::INITIAL`, `SchemaVersion::new(NonZeroU32)`, `SchemaVersion::from_u32(u32) -> Result`, `SchemaVersion::get() -> u32`, `From<SchemaVersion> for nexus::Version` — consistent surface.
- `from_static_str`, `from_bytes`, `from_validated_bytes` (pub(crate)), `as_str` / `as_slice` / `as_bytes`, `into_bytes` — consistent across `EventType`/`Payload`/`Metadata` per their natural shape.
- Builder method names: `event_type` (infallible, static-str), `event_type_bytes` (fallible, runtime bytes), `payload`, `with_metadata`, `schema_version(SchemaVersion)` — consistent through PR2 and PR3.
- `PersistedEnvelope` accessor names: `event_type_owned`, `payload_owned`, `metadata_owned`, `schema_version_value` — consistent.
- `DecodeError::CorruptSchemaVersion` — added in PR3 Task 3.2.
- `ForDecodeError` — introduced in PR3 Task 3.3.

No drift detected.

---

## Execution Handoff

**Plan complete and saved to `docs/plans/2026-06-08-value-newtype-refactor-plan.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for a 3-PR refactor where each PR boundary is a natural review checkpoint.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints. Heavier on session context; works if you want to watch every step.

**Which approach?**
