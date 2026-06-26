# PR 1 — Kernel `ErrorId` + seal `Events::IntoIter` (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the nexus-owned `ErrorId<const N = 64>` newtype and seal `Events::IntoIter` behind a named iterator, removing `arrayvec` types from the kernel's public API — *additively*, so the workspace compiles unchanged.

**Architecture:** `ErrorId<N>` wraps a private `arrayvec::ArrayString<N>` and is constructed only via `from_display`, which truncates on a UTF-8 char boundary and appends `…` to signal loss. `EventsIntoIter<E, N>` wraps the existing `Chain<Once<E>, ArrayVecIntoIter<E, N>>` and delegates iteration, so the `arrayvec::IntoIter` type no longer appears as `Events`' associated `IntoIter`. `Id::to_label` is **left unchanged** in this PR (retyped in PR 4 once downstream consumers migrate).

**Tech Stack:** Rust 2024 (stable, no nightly features), `arrayvec` (internal only), `proptest`, integration tests under `crates/nexus/tests/kernel_tests/`.

**Scope note:** This is the spec's PR 1 (see `docs/plans/2026-06-26-seal-public-dependency-leaks-design.md`). PRs 2–4 are planned after this merges, because their edits depend on this PR's final `ErrorId` surface.

**Workflow notes:**
- Branch from `main` (e.g. `feat/208-pr1-kernel-errorid`).
- Each `git commit` triggers the pre-commit hook, which runs the full `nix flake check` gate. **Do not run the gate by hand first** — let the hook run it.
- The flake clippy is `--lib` only; run `nix develop -c cargo clippy --all-targets` by hand if you touch test/bench/example targets (this PR's tests are integration tests, covered by nextest).
- `git add` new files before committing — `nix flake check` fails on untracked source/test modules.

---

### Task 1: `ErrorId<const N>` newtype

**Files:**
- Create: `crates/nexus/src/error_id.rs`
- Create: `crates/nexus/tests/kernel_tests/error_id_tests.rs`
- Modify: `crates/nexus/tests/kernel.rs` (register the test module)
- Modify: `crates/nexus/src/lib.rs` (declare module + re-export)

- [ ] **Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel_tests/error_id_tests.rs`:

```rust
//! Tests for `nexus::ErrorId` — the bounded, truncation-signalling
//! diagnostic label that seals `arrayvec` out of the public API.

use nexus::ErrorId;
use proptest::prelude::*;

const ELLIPSIS: char = '…';

// ── Sequence/protocol: construct → read → compare ──────────────────────────

#[test]
fn short_value_is_stored_verbatim() {
    let id = ErrorId::<64>::from_display(&"order-42");
    assert_eq!(id.as_str(), "order-42");
    assert_eq!(id.len(), 8);
    assert!(!id.as_str().ends_with(ELLIPSIS));
}

#[test]
fn empty_value_is_empty() {
    let id = ErrorId::<64>::from_display(&"");
    assert!(id.is_empty());
    assert_eq!(id.as_str(), "");
}

#[test]
fn display_matches_as_str() {
    let id = ErrorId::<64>::from_display(&"stream-x");
    assert_eq!(format!("{id}"), "stream-x");
}

#[test]
fn equal_inputs_are_equal_and_hash_equal() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let a = ErrorId::<64>::from_display(&"same");
    let b = ErrorId::<64>::from_display(&"same");
    assert_eq!(a, b);

    let mut ha = DefaultHasher::new();
    let mut hb = DefaultHasher::new();
    a.hash(&mut ha);
    b.hash(&mut hb);
    assert_eq!(ha.finish(), hb.finish());
}

#[test]
fn default_is_empty() {
    let id = ErrorId::<64>::default();
    assert!(id.is_empty());
}

// ── Defensive boundary: lengths {0, 1, N-1, N, N+1} ────────────────────────

#[test]
fn exactly_capacity_is_not_truncated() {
    let input = "a".repeat(64);
    let id = ErrorId::<64>::from_display(&input);
    assert_eq!(id.as_str(), input);
    assert_eq!(id.len(), 64);
    assert!(!id.as_str().ends_with(ELLIPSIS));
}

#[test]
fn one_below_capacity_is_not_truncated() {
    let input = "a".repeat(63);
    let id = ErrorId::<64>::from_display(&input);
    assert_eq!(id.as_str(), input);
}

#[test]
fn one_above_capacity_is_truncated_with_ellipsis() {
    let input = "a".repeat(65);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
    assert!(id.as_str().starts_with("aaa"));
}

#[test]
fn far_above_capacity_is_truncated_with_ellipsis() {
    let input = "z".repeat(1000);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
}

// ── Defensive boundary: multi-byte char straddling the cap ─────────────────

#[test]
fn multibyte_char_at_boundary_does_not_panic_and_stays_valid() {
    // "€" is 3 bytes (U+20AC). 30 of them = 90 bytes; cap 64 falls
    // mid-character. Must not panic, must be valid UTF-8 (guaranteed by
    // &str), and must end with the ellipsis marker.
    let input = "€".repeat(30);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
    // No assertion on exact bytes — only that it is a valid, bounded label.
}

// ── Custom width (the fjall reason case uses N = 128) ──────────────────────

#[test]
fn custom_width_truncates_at_its_own_cap() {
    let input = "r".repeat(200);
    let id = ErrorId::<128>::from_display(&input);
    assert!(id.len() <= 128);
    assert!(id.as_str().ends_with(ELLIPSIS));
}

// ── Property: bounded, valid, verbatim-when-fits ───────────────────────────

proptest! {
    // Strategy includes the boundary lengths explicitly (rule 8).
    #[test]
    fn never_panics_and_stays_within_cap(
        input in prop_oneof![
            Just(String::new()),
            "[a-z]{1}",
            "[a-z]{31}",
            "[a-z]{32}",
            "[a-z]{33}",
            "\\PC{0,500}",   // arbitrary non-control unicode, any length
        ]
    ) {
        let id = ErrorId::<32>::from_display(&input);
        prop_assert!(id.as_str().len() <= 32);
        if input.len() <= 32 {
            prop_assert_eq!(id.as_str(), input.as_str());
        }
    }
}
```

Register it in `crates/nexus/tests/kernel.rs` — add alongside the existing `#[path]` modules (keep the list alphabetical: insert after the `error_tests` entry):

```rust
#[path = "kernel_tests/error_id_tests.rs"]
mod error_id_tests;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus --test kernel error_id_tests`
Expected: FAIL — compile error, `unresolved import nexus::ErrorId` / `cannot find type ErrorId`.

- [ ] **Step 3: Write the implementation**

Create `crates/nexus/src/error_id.rs`:

```rust
//! [`ErrorId`] — a bounded, stack-allocated diagnostic label that keeps
//! `arrayvec::ArrayString` out of the public API.
//!
//! Error types across the kernel, store, and fjall adapter carry short
//! identifier / reason strings (stream ids, failure reasons) sized for
//! IoT / embedded targets — no heap allocation on error paths. Exposing
//! `ArrayString<N>` in those public fields would couple our SemVer to
//! `arrayvec` 0.x: a major bump there would be a breaking change here.
//! `ErrorId<N>` carries the same bytes without leaking the dependency.
//!
//! Construction is truncation-aware: [`ErrorId::from_display`] fills the
//! buffer and, if the source did not fit, replaces the tail with `…`
//! (U+2026) on a char boundary — visually signalling truncation. Silent
//! truncation (the old `to_label` / `reason_label` behaviour) is not
//! offered, so there is no `From<&str>` back door.

use core::fmt::{self, Debug, Display, Write};

use arrayvec::ArrayString;

/// Default capacity for an [`ErrorId`]: 64 bytes, matching stream-id labels.
pub const DEFAULT_ERROR_ID_CAP: usize = 64;

/// Truncation marker appended when a source value overflows the buffer.
/// `…` (U+2026) is 3 bytes in UTF-8.
const ELLIPSIS: &str = "…";

/// Largest byte position at or before `limit` that sits on a UTF-8 char
/// boundary in `s`. Returns `s.len()` when `limit >= s.len()`, and 0 when
/// no in-range boundary exists.
const fn truncate_at_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut pos = limit;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// A bounded, stack-allocated diagnostic label of at most `N` UTF-8 bytes.
///
/// Construct via [`ErrorId::from_display`]; longer values are truncated on a
/// char boundary and suffixed with `…`. `N` defaults to
/// [`DEFAULT_ERROR_ID_CAP`] (64).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ErrorId<const N: usize = DEFAULT_ERROR_ID_CAP> {
    inner: ArrayString<N>,
}

impl<const N: usize> ErrorId<N> {
    /// An empty label.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: ArrayString::new(),
        }
    }

    /// Build a label from a [`Display`] value, truncating on a char
    /// boundary with a trailing `…` if it exceeds `N` bytes.
    #[must_use]
    pub fn from_display(value: &impl Display) -> Self {
        let mut filler = Filler::<N>::new();
        // `Filler::write_str` always returns `Ok`, so `write!` cannot fail.
        let _ = write!(filler, "{value}");
        filler.finish()
    }

    /// The label as a string slice. Always valid UTF-8, length `<= N`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    /// Number of bytes in the label.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the label is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<const N: usize> Default for ErrorId<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Display for ErrorId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> Debug for ErrorId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print like a string literal so error `{:?}` output reads cleanly.
        write!(f, "{:?}", self.as_str())
    }
}

/// Fills an `ArrayString<N>` from `fmt::Write` chunks, recording whether
/// any input was dropped so the tail can be marked with `…`.
struct Filler<const N: usize> {
    buf: ArrayString<N>,
    truncated: bool,
}

impl<const N: usize> Filler<N> {
    fn new() -> Self {
        Self {
            buf: ArrayString::new(),
            truncated: false,
        }
    }

    fn finish(self) -> ErrorId<N> {
        if self.truncated {
            ErrorId {
                inner: append_ellipsis(self.buf),
            }
        } else {
            ErrorId { inner: self.buf }
        }
    }
}

impl<const N: usize> Write for Filler<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.truncated {
            // Already overflowed; nothing more fits.
            return Ok(());
        }
        let remaining = N - self.buf.len();
        if s.len() <= remaining {
            self.buf.try_push_str(s).ok();
            return Ok(());
        }
        let end = truncate_at_char_boundary(s, remaining);
        self.buf.try_push_str(&s[..end]).ok();
        self.truncated = true;
        Ok(())
    }
}

/// Replace the tail of `buf` with `…` on a char boundary so the result is
/// `<= N` bytes. If `N` cannot hold the 3-byte marker (never the case for
/// the 64/128 widths in use), the buffer is returned without a marker.
fn append_ellipsis<const N: usize>(mut buf: ArrayString<N>) -> ArrayString<N> {
    if N < ELLIPSIS.len() {
        return buf;
    }
    let target = N - ELLIPSIS.len();
    let cut = truncate_at_char_boundary(buf.as_str(), target);
    buf.truncate(cut);
    // `cut <= N - 3` and the marker is 3 bytes, so this always fits.
    buf.try_push_str(ELLIPSIS).ok();
    buf
}
```

Wire it into `crates/nexus/src/lib.rs` — add the module declaration after `mod error;`:

```rust
mod error_id;
```

and the re-export after `pub use error::KernelError;`:

```rust
pub use error_id::{DEFAULT_ERROR_ID_CAP, ErrorId};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus --test kernel error_id_tests`
Expected: PASS — all unit tests and the `never_panics_and_stays_within_cap` proptest green.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus/src/error_id.rs crates/nexus/src/lib.rs \
        crates/nexus/tests/kernel_tests/error_id_tests.rs crates/nexus/tests/kernel.rs
git commit -m "feat(kernel): add ErrorId<N> newtype sealing arrayvec from public API (#208)"
```

(The pre-commit hook runs `nix flake check`. Let it run.)

---

### Task 2: Seal `Events::IntoIter` behind `EventsIntoIter`

**Files:**
- Modify: `crates/nexus/src/events.rs:109-116` (replace the owning `IntoIterator` impl)
- Modify: `crates/nexus/src/lib.rs` (re-export `EventsIntoIter`)
- Create: `crates/nexus/tests/kernel_tests/events_into_iter_tests.rs`
- Modify: `crates/nexus/tests/kernel.rs` (register the test module)

- [ ] **Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel_tests/events_into_iter_tests.rs`:

```rust
//! Tests that the owning iterator of `Events` is the named, leak-free
//! `EventsIntoIter` type and preserves Chain's iteration capabilities.

use nexus::{DomainEvent, Events, EventsIntoIter, events};

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestEvent {
    A,
    B,
    C,
}

impl nexus::Message for TestEvent {}

impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }
}

#[test]
fn into_iter_returns_the_named_sealed_type() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    // The associated `IntoIter` is `EventsIntoIter`, not an arrayvec type.
    let it: EventsIntoIter<TestEvent, 2> = evts.into_iter();
    let collected: Vec<TestEvent> = it.collect();
    assert_eq!(collected, vec![TestEvent::A, TestEvent::B, TestEvent::C]);
}

#[test]
fn yields_first_then_rest_in_order() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B];
    let collected: Vec<TestEvent> = evts.into_iter().collect();
    assert_eq!(collected, vec![TestEvent::A, TestEvent::B]);
}

#[test]
fn is_double_ended() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    let reversed: Vec<TestEvent> = evts.into_iter().rev().collect();
    assert_eq!(reversed, vec![TestEvent::C, TestEvent::B, TestEvent::A]);
}

#[test]
fn is_fused_after_exhaustion() {
    let evts: Events<TestEvent, 0> = events![TestEvent::A];
    let mut it = evts.into_iter();
    assert_eq!(it.next(), Some(TestEvent::A));
    assert_eq!(it.next(), None);
    assert_eq!(it.next(), None);
}

#[test]
fn size_hint_counts_all_events() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    let it = evts.into_iter();
    assert_eq!(it.size_hint(), (3, Some(3)));
}
```

Register it in `crates/nexus/tests/kernel.rs` (insert after the `error_tests` / `error_id_tests` entries, keeping alphabetical order — `events_into_iter_tests` precedes `events_tests`):

```rust
#[path = "kernel_tests/events_into_iter_tests.rs"]
mod events_into_iter_tests;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus --test kernel events_into_iter_tests`
Expected: FAIL — compile error, `cannot find type EventsIntoIter in crate nexus`.

- [ ] **Step 3: Write the implementation**

In `crates/nexus/src/events.rs`, replace the owning `IntoIterator` impl (lines 109–116) with the named newtype plus its delegating impls:

```rust
/// Owning iterator over [`Events`], yielding `first` then each event in
/// `rest`. A named newtype wrapping the concrete `Chain<Once, _>` so the
/// `arrayvec::IntoIter` type does not appear in the public API as
/// `Events`' associated `IntoIter`.
#[derive(Debug)]
pub struct EventsIntoIter<E: DomainEvent, const N: usize> {
    inner: Chain<Once<E>, ArrayVecIntoIter<E, N>>,
}

impl<E: DomainEvent, const N: usize> Iterator for EventsIntoIter<E, N> {
    type Item = E;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<E: DomainEvent, const N: usize> DoubleEndedIterator for EventsIntoIter<E, N> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
}

impl<E: DomainEvent, const N: usize> core::iter::FusedIterator for EventsIntoIter<E, N> {}

impl<E: DomainEvent, const N: usize> IntoIterator for Events<E, N> {
    type Item = E;
    type IntoIter = EventsIntoIter<E, N>;

    fn into_iter(self) -> Self::IntoIter {
        EventsIntoIter {
            inner: once(self.first).chain(self.rest),
        }
    }
}
```

(The existing `use arrayvec::{ArrayVec, IntoIter as ArrayVecIntoIter};` and `use core::iter::{Chain, Once, once};` at the top of the file already cover every name used here — no import changes needed.)

Re-export the new type from `crates/nexus/src/lib.rs` by extending the existing events re-export:

```rust
pub use events::{Events, EventsIntoIter};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus --test kernel events_into_iter_tests`
Expected: PASS.

- [ ] **Step 5: Run the full kernel suite to confirm no regression**

Run: `nix develop -c cargo test -p nexus`
Expected: PASS — existing `events_tests` and all other kernel tests still green.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus/src/events.rs crates/nexus/src/lib.rs \
        crates/nexus/tests/kernel_tests/events_into_iter_tests.rs crates/nexus/tests/kernel.rs
git commit -m "feat(kernel): seal Events::IntoIter behind named EventsIntoIter (#208)"
```

---

## Self-review

**Spec coverage (PR 1 portion):**
- `ErrorId<const N = 64>`, kernel-owned, const-generic width → Task 1. ✓
- Truncation `…` signal on char boundary → Task 1 impl + boundary/multibyte tests. ✓
- No `From<&str>` back door → omitted by construction; `from_display` is the only constructor. ✓
- `no_std`-friendly (`core::fmt` + `arrayvec` core methods) → Task 1. ✓
- Seal `Events::IntoIter` via named newtype (stable Rust forces this) → Task 2. ✓
- `to_label` deliberately untouched in PR 1 → noted; deferred to PR 4. ✓
- 4-category tests on `ErrorId` (sequence, boundary incl. {0,1,N-1,N,N+1}, multibyte; linearizability n/a for an immutable value) → Task 1. ✓

**Placeholder scan:** none — every step has full code and exact commands.

**Type consistency:** `ErrorId<N>` / `from_display` / `as_str` / `len` / `is_empty` / `default` used identically across impl and tests. `EventsIntoIter<E, N>` field `inner` matches its impls. `DEFAULT_ERROR_ID_CAP` defined and re-exported.

**Out of scope for PR 1 (tracked in the design doc):** store/fjall error-field retypes (PR 2/3), `pub use bytes`, `futures_core` rebind (PR 2), `ProjectedIntents::IntoIter` seal (PR 2), `Id::to_label` retype (PR 4), fjall versioning (#221).
