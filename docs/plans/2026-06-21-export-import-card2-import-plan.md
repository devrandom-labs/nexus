# Export/Import Card 2 — `EventImporter` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the picky, halt-not-skip, per-stream `EventImporter` for `nexus-store`, with both `Atomicity::PerStream` and `Atomicity::WholeChunk` modes, behind feature `import`.

**Architecture:** One `EventImporter: RawEventStore + AtomicAppend` trait, blanket-impl'd for every store that is both. All routing/halt/report logic lives in the blanket impl; the only per-adapter code is the new `AtomicAppend` transaction primitive. A pure per-section planner (decode first block → longest contiguous run → halt reason) is shared by both atomicity modes; only the head-check and the write differ. Imported events keep their origin versions (version-preserving); the store restamps a fresh `global_seq`.

**Tech Stack:** Rust 2024, `futures`, `bytes::Bytes`, `thiserror`, `proptest`, `static_assertions`, `tokio`. Design doc: `docs/plans/2026-06-21-export-import-card2-import-design.md`.

**Conventions for every commit:**
- `nix develop -c cargo fmt --all` before staging.
- `git add` all new/changed files (the pre-commit hook runs `nix flake check`, which ignores untracked files → false failures). **Do NOT pre-run `nix flake check` by hand** — the hook runs it.
- Conventional commit `feat(store): …`, ending with the trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Local verification before each commit (these are NOT the gate, just fast feedback):
  - `nix develop -c cargo nextest run -p nexus-store --features export,import,testing`
  - `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`

---

## File Structure

- `crates/nexus-store/src/envelope.rs` — add `pub(crate) PendingEnvelope::from_persisted`.
- `crates/nexus-store/src/import.rs` — add `StreamSection`, `ImportBlock`, `AtomicAppend`, `PlannedAppend`, `AtomicAppendError`; the pure planner (`plan_section`, `SectionPlan`, `Halt`, `PlanError`); the blanket `EventImporter` impl (PerStream + WholeChunk); replace the provisional trait signature; delete the `NoopStore` scaffold; all behavioral + property tests inline (mirrors `export.rs`).
- `crates/nexus-store/src/testing.rs` — `impl AtomicAppend for InMemoryStore`, gated `#[cfg(feature = "import")]`.
- `crates/nexus-store/src/lib.rs` — re-export the user-facing new items under `#[cfg(feature = "import")]`.

---

## Task 1: Foundations — `from_persisted` + `AtomicAppend` primitive

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`
- Modify: `crates/nexus-store/src/import.rs`
- Modify: `crates/nexus-store/src/testing.rs`

- [ ] **Step 1: Write the failing test for `from_persisted`**

In `crates/nexus-store/src/envelope.rs`, add to the existing `#[cfg(test)] mod tests` block (the first one, near line 588):

```rust
    #[test]
    fn from_persisted_preserves_fields_drops_global_seq_zero_copy() {
        // A read-path envelope at version 7, global_seq 99, schema 3, with meta.
        let value = Bytes::from_static(b"TYPEpayloadmeta");
        let persisted = PersistedEnvelope::try_new(
            Version::new(7).expect("nonzero"),
            crate::store::GlobalSeq::new(99).expect("nonzero"),
            value,
            crate::value::SchemaVersion::from_u32(3).expect("nonzero"),
            0..4,
            4..11,
            Some(11..15),
        )
        .expect("valid");

        let pending = PendingEnvelope::from_persisted(&persisted);

        // Every field carried verbatim (global_seq has no home on the write path).
        assert_eq!(pending.version(), persisted.version());
        assert_eq!(pending.event_type(), "TYPE");
        assert_eq!(pending.payload(), b"payload");
        assert_eq!(pending.metadata(), Some(b"meta".as_slice()));
        assert_eq!(pending.schema_version(), 3);

        // Zero-copy: the rebuilt payload aliases the same backing allocation.
        assert!(
            std::ptr::eq(pending.payload_bytes().as_ptr(), persisted.payload().as_ptr()),
            "from_persisted must reuse the Arc-shared payload, not deep-copy",
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing from_persisted_preserves -- --nocapture`
Expected: FAIL — `no function or associated item named 'from_persisted'`.

- [ ] **Step 3: Implement `from_persisted`**

In `crates/nexus-store/src/envelope.rs`, add to the `impl PendingEnvelope` block (after the accessors, around line 166):

```rust
    /// Rebuild a write-path envelope from a read-path one.
    ///
    /// Reuses the [`PersistedEnvelope`]'s already-validated value newtypes
    /// (one Arc share each — zero copy, no re-validation) and **drops
    /// `global_seq`**: the store stamps a fresh one on append. Infallible,
    /// because every field of a `PersistedEnvelope` was validated at its own
    /// construction. Used by the importer to re-append exported events.
    #[must_use]
    pub(crate) fn from_persisted(persisted: &PersistedEnvelope) -> Self {
        Self {
            version: persisted.version(),
            event_type: persisted.event_type_value(),
            schema_version: persisted.schema_version_value(),
            payload: persisted.payload_value(),
            metadata: persisted.metadata_value(),
        }
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing from_persisted_preserves`
Expected: PASS.

- [ ] **Step 5: Add the `AtomicAppend` primitive types + trait**

In `crates/nexus-store/src/import.rs`, extend the top-of-file imports. The current imports are:

```rust
use nexus::{Id, Version};
use thiserror::Error;

use crate::envelope::PersistedEnvelope;
use crate::store::RawEventStore;
```

Replace them with:

```rust
use bytes::Bytes;
use nexus::{Id, Version};
use thiserror::Error;

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::error::AppendError;
use crate::store::RawEventStore;
```

Then, immediately above the `pub trait EventImporter` definition, add:

```rust
/// One planned per-stream write for [`AtomicAppend::atomic_append_many`].
///
/// `events` is a contiguous, version-preserving run; `expected_version` is the
/// head the target stream must currently be at (`None` = the stream must be
/// fresh). Built by the importer's per-section planner.
#[derive(Debug, Clone)]
pub struct PlannedAppend<I> {
    /// The resolved target stream id.
    pub target: I,
    /// The version the target must currently be at (`None` = fresh stream).
    pub expected_version: Option<Version>,
    /// The contiguous run to append, in version order (always non-empty).
    pub events: Vec<PendingEnvelope>,
}

/// Failure of an [`AtomicAppend::atomic_append_many`] transaction.
///
/// `Conflict` is the cross-stream picky check: write `index`'s
/// `expected_version` did not match the target's actual head (`actual`). The
/// whole transaction is rolled back — nothing landed. `Store` is an
/// adapter-level failure. Distinct domains, distinct variants (CLAUDE rule 3).
#[derive(Debug, Error)]
pub enum AtomicAppendError<E> {
    /// Write at `index` had a head mismatch; `actual` is the target's real head.
    #[error("atomic append conflict at write {index}: actual head {actual:?}")]
    Conflict {
        index: usize,
        actual: Option<Version>,
    },
    /// Adapter-level failure (I/O, encoding, global-seq overflow, …).
    #[error("atomic append store error: {0}")]
    Store(#[source] E),
}

/// Adapter capability: commit several per-stream runs in **one** atomic
/// transaction.
///
/// Either every run lands or none do. This is the primitive
/// [`Atomicity::WholeChunk`] needs and that [`RawEventStore::append`]
/// (per-stream only) cannot provide. Adapters implement it with a real
/// transaction (fjall cross-partition `write_tx`, postgres `BEGIN..COMMIT`,
/// `InMemoryStore` its single mutex).
///
/// # Contract
///
/// - For each write, the target's actual head must equal `expected_version`,
///   else the whole transaction aborts with [`AtomicAppendError::Conflict`]
///   carrying that write's `index` and the target's real head.
/// - Each write's `events` must be a contiguous run starting at
///   `expected_version + 1`. The caller (the importer's planner) guarantees
///   this; implementations validate defensively at their own boundary.
/// - On any failure, **no** write is applied.
pub trait AtomicAppend: RawEventStore {
    /// Append every write atomically. See the trait contract.
    fn atomic_append_many<I: Id>(
        &self,
        writes: &[PlannedAppend<I>],
    ) -> impl std::future::Future<Output = Result<(), AtomicAppendError<Self::Error>>> + Send;
}
```

Note: `Bytes`, `PendingEnvelope`, and `AppendError` are imported now because
later tasks use them; if clippy flags them as unused before then, that is
expected mid-task and clears by Task 4.

- [ ] **Step 6: Write the failing tests for `atomic_append_many`**

In `crates/nexus-store/src/import.rs`, the existing `#[cfg(test)] mod tests` block currently pins frozen types and the `NoopStore` scaffold. Add these tests to that block (do not remove anything yet). Add to its `use super::*;` siblings: `use crate::testing::InMemoryStore;`, `use crate::envelope::pending_envelope;`, `use futures::StreamExt;`, `use bytes::Bytes;`.

Add a shared test `Id` and helpers near the top of the test module:

```rust
    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    fn tid(s: &str) -> Tid {
        Tid(s.to_owned())
    }

    fn pending(v: u64, payload: &[u8]) -> crate::envelope::PendingEnvelope {
        pending_envelope(v(v))
            .event_type("E")
            .payload(Bytes::copy_from_slice(payload))
            .expect("valid payload")
            .build()
    }

    fn planned(target: &str, expected: Option<u64>, versions: &[u64]) -> PlannedAppend<Tid> {
        PlannedAppend {
            target: tid(target),
            expected_version: expected.and_then(Version::new),
            events: versions.iter().map(|n| pending(*n, b"p")).collect(),
        }
    }

    async fn head_len(store: &InMemoryStore, id: &Tid) -> usize {
        store
            .read_stream(id, Version::INITIAL)
            .await
            .expect("read opens")
            .filter_map(|r| async move { r.ok() })
            .count()
            .await
    }
```

Then the behavior tests:

```rust
    #[tokio::test]
    async fn atomic_append_many_commits_all_writes() {
        use crate::import::AtomicAppend;
        let store = InMemoryStore::new();
        let writes = vec![
            planned("a", None, &[1, 2]),
            planned("b", None, &[1]),
        ];
        store.atomic_append_many(&writes).await.expect("commits");
        assert_eq!(head_len(&store, &tid("a")).await, 2);
        assert_eq!(head_len(&store, &tid("b")).await, 1);
    }

    #[tokio::test]
    async fn atomic_append_many_rolls_back_all_on_one_conflict() {
        use crate::import::AtomicAppend;
        let store = InMemoryStore::new();
        // Pre-seed "b" to v1 so the second write (expecting fresh) conflicts.
        store
            .append(&tid("b"), None, &[pending(1, b"seed")])
            .await
            .expect("seed");

        let writes = vec![
            planned("a", None, &[1, 2]), // would be fine alone
            planned("b", None, &[1]),    // conflicts: "b" is already at v1
        ];
        let err = store
            .atomic_append_many(&writes)
            .await
            .expect_err("must conflict");
        match err {
            AtomicAppendError::Conflict { index, actual } => {
                assert_eq!(index, 1);
                assert_eq!(actual, Version::new(1));
            }
            AtomicAppendError::Store(_) => panic!("expected Conflict, got Store"),
        }
        // Atomicity: NOTHING landed — "a" must still be empty.
        assert_eq!(head_len(&store, &tid("a")).await, 0, "rolled back");
        assert_eq!(head_len(&store, &tid("b")).await, 1, "unchanged");
    }
```

Add `#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test code asserts exact values")]` and `#![allow(clippy::panic, reason = "test assertion")]` at the top of the test module if not already present (mirror `export.rs`'s test-module allow block; `export.rs` uses a `#[cfg(test)] #[allow(...)] mod tests` outer attribute — match that form).

- [ ] **Step 7: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing atomic_append_many -- --nocapture`
Expected: FAIL — `no method named atomic_append_many found for struct InMemoryStore` (the impl doesn't exist yet).

- [ ] **Step 8: Implement `AtomicAppend` for `InMemoryStore`**

In `crates/nexus-store/src/testing.rs`, after the `StreamLister` impl block (around line 533), add:

```rust
// ═══════════════════════════════════════════════════════════════════════════
// AtomicAppend — cross-stream atomic commit (import support, issue #145)
// ═══════════════════════════════════════════════════════════════════════════

/// Commit several per-stream runs in one atomic critical section.
///
/// Holds the `streams` lock for the whole operation — validate every write's
/// head first (no mutation), then encode + apply all — so a half-write is
/// unrepresentable and no concurrent `append` can interleave. Mirrors
/// `append`'s lock order (`streams` → `next_global_seq` → `global_index`).
#[cfg(feature = "import")]
impl crate::import::AtomicAppend for InMemoryStore {
    async fn atomic_append_many<I: Id>(
        &self,
        writes: &[crate::import::PlannedAppend<I>],
    ) -> Result<(), crate::import::AtomicAppendError<Self::Error>> {
        use crate::import::AtomicAppendError;

        let mut guard = self.streams.lock().await;

        // Phase 1 — validate every head and run shape; NO mutation.
        for (index, w) in writes.iter().enumerate() {
            let key = w.target.to_string();
            let actual_raw = u64::try_from(guard.get(&key).map_or(0, Vec::len)).unwrap_or(u64::MAX);
            let expected_raw = w.expected_version.map_or(0, Version::as_u64);
            if actual_raw != expected_raw {
                return Err(AtomicAppendError::Conflict {
                    index,
                    actual: Version::new(actual_raw),
                });
            }
            // Defensive (CLAUDE: each crate validates at its own boundary): the
            // run must be strictly sequential from expected+1. The importer's
            // planner guarantees this; a violation here is a caller bug, mapped
            // to a Conflict on the offending write.
            for (i, env) in w.events.iter().enumerate() {
                let offset = u64::try_from(i).unwrap_or(u64::MAX);
                let want = expected_raw
                    .checked_add(1)
                    .and_then(|base| base.checked_add(offset))
                    .ok_or(AtomicAppendError::Store(InMemoryStoreError::VersionOverflow))?;
                if env.version().as_u64() != want {
                    return Err(AtomicAppendError::Conflict {
                        index,
                        actual: Version::new(actual_raw),
                    });
                }
            }
        }

        // Phase 2 — assign global_seqs and stage frames (still no store mutation).
        let mut counter = self.next_global_seq.lock().await;
        let mut seq = *counter;
        let mut staged_streams: Vec<(String, Vec<StoredFrame>)> = Vec::with_capacity(writes.len());
        let mut staged_global: Vec<(u64, StoredFrame)> = Vec::new();
        for w in writes {
            let mut frames = Vec::with_capacity(w.events.len());
            for env in &w.events {
                let frame = encode_pending_to_frame(env, seq).map_err(|e| match e {
                    AppendError::Store(s) => AtomicAppendError::Store(s),
                    AppendError::Conflict { .. } => {
                        AtomicAppendError::Store(InMemoryStoreError::VersionOverflow)
                    }
                })?;
                staged_global.push((seq.as_u64(), frame.clone()));
                frames.push(frame);
                seq = seq
                    .next()
                    .ok_or(AtomicAppendError::Store(InMemoryStoreError::GlobalSeqOverflow))?;
            }
            staged_streams.push((w.target.to_string(), frames));
        }
        *counter = seq;
        drop(counter);

        // Phase 3 — commit: global index first, then per-stream (same critical
        // section, streams lock still held throughout).
        {
            let mut gidx = self.global_index.lock().await;
            for (s, frame) in &staged_global {
                gidx.insert(*s, frame.clone());
            }
        }
        for (key, frames) in staged_streams {
            guard.entry(key).or_default().extend(frames);
        }
        drop(guard);

        // Wake subscribers parked on each touched stream + the $all notifier.
        for w in writes {
            if !w.events.is_empty() {
                self.notifiers.wake(w.target.as_ref());
            }
        }
        self.notifiers.wake_all();
        Ok(())
    }
}
```

`encode_pending_to_frame`, `StoredFrame`, `InMemoryStoreError`, `AppendError`,
and `self.notifiers` / `self.next_global_seq` / `self.global_index` are all
already in scope in `testing.rs`.

- [ ] **Step 9: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing atomic_append_many`
Expected: PASS (both tests).

- [ ] **Step 10: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/envelope.rs crates/nexus-store/src/import.rs crates/nexus-store/src/testing.rs docs/plans/2026-06-21-export-import-card2-import-design.md docs/plans/2026-06-21-export-import-card2-import-plan.md
git commit -m "$(cat <<'EOF'
feat(store): import foundations — PendingEnvelope::from_persisted + AtomicAppend

Card 2 (#145) groundwork: an infallible, zero-copy PersistedEnvelope →
PendingEnvelope rebuild (drops global_seq for restamping) and the AtomicAppend
cross-stream transaction primitive that WholeChunk import needs, with an
InMemoryStore impl (single-mutex, validate-all-then-apply-all).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Input types + trait reshape

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Add `StreamSection` + `ImportBlock`**

In `crates/nexus-store/src/import.rs`, immediately above `PlannedAppend` (added in Task 1), add:

```rust
/// One origin stream's events, as decoded by the backup box (Card 3).
///
/// The origin stream id is recorded **once** per section (export stamps no
/// per-event id); the importer maps it to a target via the `route` closure.
#[derive(Debug, Clone)]
pub struct StreamSection {
    /// The origin stream id, exactly as the box recorded it.
    pub origin: Bytes,
    /// The section's blocks, in version order as the box laid them down.
    pub blocks: Vec<ImportBlock>,
}

/// One block within a [`StreamSection`].
#[derive(Debug, Clone)]
pub enum ImportBlock {
    /// A block whose per-block checksum passed and decoded to an event.
    Event(PersistedEnvelope),
    /// A block whose per-block checksum **failed**. Carries nothing: a failed
    /// checksum means the decoded header — including its version — cannot be
    /// trusted, which is exactly why [`StreamOutcome::Corrupt`] omits the
    /// version.
    Corrupt,
}
```

- [ ] **Step 2: Replace the provisional `EventImporter` trait**

In `crates/nexus-store/src/import.rs`, the current trait is:

```rust
pub trait EventImporter: RawEventStore {
    fn import<I, R>(
        &self,
        events: &[PersistedEnvelope],
        route: R,
        atomicity: Atomicity,
    ) -> impl std::future::Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
    where
        I: Id,
        R: Fn(&[u8]) -> I + Send;
}
```

Replace the whole trait (keep the doc comment above it, updating the input
wording) with:

```rust
pub trait EventImporter: RawEventStore + AtomicAppend {
    /// Import per-stream sections onto caller-routed target streams.
    fn import<I, R>(
        &self,
        sections: &[StreamSection],
        route: R,
        atomicity: Atomicity,
    ) -> impl std::future::Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
    where
        I: Id,
        R: Fn(&[u8]) -> I + Send;
}
```

Update the trait's doc comment: change the `NOTE` paragraph (about the
provisional `&[PersistedEnvelope]`) to state that input is now per-stream
[`StreamSection`]s carrying [`ImportBlock`]s, and that `route` maps
`section.origin` → target id `I`.

- [ ] **Step 3: Delete the `NoopStore` scaffold**

In the `#[cfg(test)] mod tests` block, delete the entire scaffold that the
Card-0 comment marks "pin the frozen trait surface": the `NoopError` struct,
the `NoopStore` struct, its `impl RawEventStore for NoopStore`, its
`impl EventImporter for NoopStore`, and the `assert_impl_all!(NoopStore: …)`.
This manual `EventImporter` impl now collides with the blanket impl added in
Task 4. **Keep every frozen-type test** (`StreamOutcome`, `ImportReport`,
`StreamReport`, `AbortReason`, `ImportError`, `Atomicity`).

Also delete the now-unused test import `use crate::store::GlobalSeq;` if the
scaffold was its only user (the frozen-type tests do not use it). Keep
`use crate::envelope::PendingEnvelope;` / `PersistedEnvelope` only if still
referenced; remove if not (clippy will tell you).

- [ ] **Step 4: Verify it compiles and frozen tests still pass**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing --lib import::tests`
Expected: PASS — all frozen-type tests green; no `NoopStore` references remain.

(The trait now has no impl yet; that is fine — a trait with no implementors compiles.)

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
feat(store): import input is per-stream sections; EventImporter: +AtomicAppend

Replaces the Card-0 provisional &[PersistedEnvelope] signature with per-stream
StreamSection/ImportBlock input (origin recorded once, Corrupt is a first-class
block), and makes AtomicAppend a supertrait so one import() honours both
atomicity modes. Deletes the NoopStore frozen-surface scaffold ahead of the
blanket impl; frozen-type tests retained.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: The pure per-section planner

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Write the failing planner tests**

In the `import.rs` test module, add (the `persisted`/`section`/`ev` helpers are
defined here):

```rust
    fn persisted(v: u64, payload: &[u8]) -> PersistedEnvelope {
        use crate::store::GlobalSeq;
        use crate::value::SchemaVersion;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"E");
        buf.extend_from_slice(payload);
        let et_end = 1u32;
        let pl_end = et_end + u32::try_from(payload.len()).expect("payload fits u32");
        PersistedEnvelope::try_new(
            v(v),
            GlobalSeq::new(v).expect("nonzero"), // arbitrary; import ignores global_seq
            Bytes::from(buf),
            SchemaVersion::INITIAL,
            0..et_end,
            et_end..pl_end,
            None,
        )
        .expect("valid persisted envelope")
    }
    fn evt(version: u64) -> ImportBlock {
        ImportBlock::Event(persisted(version, b"p"))
    }
    fn section(origin: &str, blocks: Vec<ImportBlock>) -> StreamSection {
        StreamSection {
            origin: Bytes::copy_from_slice(origin.as_bytes()),
            blocks,
        }
    }

    #[test]
    fn plan_empty_section_is_empty() {
        assert!(matches!(
            plan_section(&section("s", vec![])),
            Ok(SectionPlan::Empty)
        ));
    }

    #[test]
    fn plan_first_block_corrupt_is_first_corrupt() {
        let s = section("s", vec![ImportBlock::Corrupt, evt(1)]);
        assert!(matches!(plan_section(&s), Ok(SectionPlan::FirstCorrupt)));
    }

    #[test]
    fn plan_contiguous_run_from_one_is_complete() {
        let s = section("s", vec![evt(1), evt(2), evt(3)]);
        let plan = plan_section(&s).expect("plans");
        match plan {
            SectionPlan::Run { first, expected_version, last, halt, events } => {
                assert_eq!(first, v(1));
                assert_eq!(expected_version, None); // first == 1 → fresh stream
                assert_eq!(last, v(3));
                assert_eq!(events.len(), 3);
                assert!(matches!(halt, Halt::Complete));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn plan_run_from_midstream_sets_expected_to_first_minus_one() {
        let s = section("s", vec![evt(3), evt(4)]);
        match plan_section(&s).expect("plans") {
            SectionPlan::Run { first, expected_version, last, halt, .. } => {
                assert_eq!(first, v(3));
                assert_eq!(expected_version, Some(v(2)));
                assert_eq!(last, v(4));
                assert!(matches!(halt, Halt::Complete));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn plan_internal_gap_halts_with_got() {
        // v3,v4,v6 → run [3,4], halt at the gap (got = 6).
        let s = section("s", vec![evt(3), evt(4), evt(6)]);
        match plan_section(&s).expect("plans") {
            SectionPlan::Run { last, halt, events, .. } => {
                assert_eq!(last, v(4));
                assert_eq!(events.len(), 2);
                assert!(matches!(halt, Halt::Gap { got } if got == v(6)));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn plan_internal_corrupt_halts_corrupt() {
        let s = section("s", vec![evt(1), evt(2), ImportBlock::Corrupt, evt(3)]);
        match plan_section(&s).expect("plans") {
            SectionPlan::Run { last, halt, events, .. } => {
                assert_eq!(last, v(2));
                assert_eq!(events.len(), 2);
                assert!(matches!(halt, Halt::Corrupt));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn plan_overflow_building_run_errors() {
        // last == u64::MAX with a following block forces prev.next() overflow.
        let s = section("s", vec![evt(u64::MAX), evt(1)]);
        assert!(matches!(plan_section(&s), Err(PlanError::VersionOverflow)));
    }
```

`SectionPlan` must derive `Debug` for the `panic!("… {other:?}")` arms.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing plan_ -- --nocapture`
Expected: FAIL — `cannot find function 'plan_section'` / `SectionPlan` / `Halt` / `PlanError`.

- [ ] **Step 3: Implement the planner**

In `crates/nexus-store/src/import.rs`, above the blanket impl region (after the
trait definitions, before the `#[cfg(test)]`), add:

```rust
/// Where a section's contiguous run stopped.
#[derive(Debug)]
enum Halt {
    /// Every block in the section was consumed.
    Complete,
    /// Stopped at a corrupt block.
    Corrupt,
    /// Stopped at a version discontinuity; `got` is the offending version.
    Gap { got: Version },
}

/// The store-independent plan for one [`StreamSection`].
#[derive(Debug)]
enum SectionPlan {
    /// No blocks — nothing to do, no report entry.
    Empty,
    /// The first block was corrupt — nothing can be appended.
    FirstCorrupt,
    /// A contiguous run to append, and how it ended.
    Run {
        /// The run's first version (used as `got` on a store conflict).
        first: Version,
        /// The head the target must be at (`None` = fresh).
        expected_version: Option<Version>,
        /// The run, rebuilt as write-path envelopes (always non-empty).
        events: Vec<PendingEnvelope>,
        /// The run's last version (where the stream lands on success).
        last: Version,
        /// Why the run stopped.
        halt: Halt,
    },
}

/// Planner failure — a stream version overflowed `u64` (NOT a conflict).
#[derive(Debug)]
enum PlanError {
    VersionOverflow,
}

/// Build a section's plan: decode the first block, accumulate the longest
/// contiguous run, and record why it stopped. Pure — no store access.
fn plan_section(section: &StreamSection) -> Result<SectionPlan, PlanError> {
    let mut blocks = section.blocks.iter();
    let Some(first_block) = blocks.next() else {
        return Ok(SectionPlan::Empty);
    };
    let first_event = match first_block {
        ImportBlock::Corrupt => return Ok(SectionPlan::FirstCorrupt),
        ImportBlock::Event(event) => event,
    };

    let first = first_event.version();
    // expected head = first - 1; first == 1 → None (fresh stream). Checked.
    let expected_version = first.as_u64().checked_sub(1).and_then(Version::new);

    let mut events = vec![PendingEnvelope::from_persisted(first_event)];
    let mut last = first;
    let halt = loop {
        let Some(block) = blocks.next() else {
            break Halt::Complete;
        };
        let event = match block {
            ImportBlock::Corrupt => break Halt::Corrupt,
            ImportBlock::Event(event) => event,
        };
        let expected_next = last.next().ok_or(PlanError::VersionOverflow)?;
        if event.version() != expected_next {
            break Halt::Gap {
                got: event.version(),
            };
        }
        events.push(PendingEnvelope::from_persisted(event));
        last = event.version();
    };

    Ok(SectionPlan::Run {
        first,
        expected_version,
        events,
        last,
        halt,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing plan_`
Expected: PASS (all planner tests).

- [ ] **Step 5: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
feat(store): pure per-section import planner (longest contiguous run + halt)

plan_section decodes the first block, accumulates the longest contiguous run,
and records the halt reason (Complete / Corrupt / Gap{got}); checked version
arithmetic surfaces VersionOverflow. Store-independent and shared by both
atomicity modes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Blanket `EventImporter` — PerStream

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Write the failing PerStream behavioral tests**

Add to the `import.rs` test module. These use `InMemoryStore` and a route
closure. Add helper:

```rust
    fn identity_route(origin: &[u8]) -> Tid {
        Tid(String::from_utf8(origin.to_vec()).expect("utf8 origin"))
    }

    async fn versions(store: &InMemoryStore, id: &Tid) -> Vec<u64> {
        store
            .read_stream(id, Version::INITIAL)
            .await
            .expect("read opens")
            .map(|r| r.expect("no read error").version().as_u64())
            .collect()
            .await
    }
```

Tests:

```rust
    #[tokio::test]
    async fn per_stream_clean_multi_stream_all_complete() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![
            section("a", vec![evt(1), evt(2), evt(3)]),
            section("b", vec![evt(1), evt(2)]),
        ];
        let report = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");

        assert!(report.all_complete());
        assert_eq!(report.streams().len(), 2);
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2, 3]);
        assert_eq!(versions(&store, &tid("b")).await, vec![1, 2]);
        assert_eq!(
            report.streams()[0].outcome,
            StreamOutcome::Complete { version: v(3) }
        );
    }

    #[tokio::test]
    async fn per_stream_partial_overlap_is_picky_rejected() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        // Target already at v3.
        for n in 1..=3 {
            store
                .append(&tid("a"), Version::new(n - 1), &[pending(n, b"seed")])
                .await
                .expect("seed");
        }
        // Section offers v2..v5 (first=2, but target wants v4 next).
        let sections = vec![section("a", vec![evt(2), evt(3), evt(4), evt(5)])];
        let report = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");

        assert_eq!(
            report.streams()[0].outcome,
            StreamOutcome::Mismatch { reached: None, got: v(2) }
        );
        // Nothing applied — still exactly v1..v3.
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn per_stream_internal_gap_applies_prefix_then_halts() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![evt(1), evt(2), evt(4)])];
        let report = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");

        assert_eq!(
            report.streams()[0].outcome,
            StreamOutcome::Mismatch { reached: Some(v(2)), got: v(4) }
        );
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2], "v4 held back");
    }

    #[tokio::test]
    async fn per_stream_mid_section_corrupt_applies_prefix_then_corrupt() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section(
            "a",
            vec![evt(1), evt(2), ImportBlock::Corrupt, evt(3)],
        )];
        let report = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");

        assert_eq!(
            report.streams()[0].outcome,
            StreamOutcome::Corrupt { reached: Some(v(2)) }
        );
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2]);
    }

    #[tokio::test]
    async fn per_stream_first_block_corrupt_reaches_none() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![ImportBlock::Corrupt, evt(1)])];
        let report = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");
        assert_eq!(
            report.streams()[0].outcome,
            StreamOutcome::Corrupt { reached: None }
        );
        assert_eq!(versions(&store, &tid("a")).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn per_stream_empty_chunk_is_empty_report() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let report = store
            .import(&[], identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");
        assert!(report.streams().is_empty());
        assert!(report.all_complete());
    }

    #[tokio::test]
    async fn per_stream_version_overflow_surfaces_error() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![evt(u64::MAX), evt(1)])];
        let err = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect_err("overflow");
        assert!(matches!(err, ImportError::VersionOverflow));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing per_stream_ -- --nocapture`
Expected: FAIL — `no method named import found for struct InMemoryStore` (blanket impl missing).

- [ ] **Step 3: Implement the PerStream path + blanket impl**

In `crates/nexus-store/src/import.rs`, after the planner, add the PerStream free
function and the blanket impl (WholeChunk filled in Task 5 — for now route it to
a stub that `unimplemented`? No — implement both call arms now but leave
WholeChunk's body to Task 5 by calling `import_whole_chunk`, which we add as a
minimal compiling stub here and flesh out in Task 5). To keep this task's tests
green without a half-built WholeChunk, implement PerStream fully and make the
blanket impl match only PerStream by delegating WholeChunk to the real function
added next task. Add **both** functions now, PerStream complete:

```rust
/// PerStream import: each section its own `append` transaction. A bad block
/// stops only its stream; the rest commit. Always returns `Ok(report)` unless
/// a genuine store error or version overflow occurs.
async fn import_per_stream<S, I, R>(
    store: &S,
    sections: &[StreamSection],
    route: R,
) -> Result<ImportReport<I>, ImportError<S::Error, I>>
where
    S: RawEventStore,
    I: Id,
    R: Fn(&[u8]) -> I + Send,
{
    let mut reports = Vec::with_capacity(sections.len());
    for section in sections {
        let target = route(section.origin.as_ref());
        let plan = match plan_section(section) {
            Ok(plan) => plan,
            Err(PlanError::VersionOverflow) => return Err(ImportError::VersionOverflow),
        };
        let outcome = match plan {
            SectionPlan::Empty => continue,
            SectionPlan::FirstCorrupt => StreamOutcome::Corrupt { reached: None },
            SectionPlan::Run {
                first,
                expected_version,
                events,
                last,
                halt,
            } => match store.append(&target, expected_version, &events).await {
                Ok(()) => match halt {
                    Halt::Complete => StreamOutcome::Complete { version: last },
                    Halt::Corrupt => StreamOutcome::Corrupt { reached: Some(last) },
                    Halt::Gap { got } => StreamOutcome::Mismatch {
                        reached: Some(last),
                        got,
                    },
                },
                Err(AppendError::Conflict { .. }) => StreamOutcome::Mismatch {
                    reached: None,
                    got: first,
                },
                Err(AppendError::Store(error)) => return Err(ImportError::Store(error)),
            },
        };
        reports.push(StreamReport {
            stream: target,
            outcome,
        });
    }
    Ok(ImportReport::new(reports))
}
```

And the blanket impl (with a temporary WholeChunk arm that compiles; Task 5
replaces the call target's body):

```rust
impl<S: RawEventStore + AtomicAppend> EventImporter for S {
    async fn import<I, R>(
        &self,
        sections: &[StreamSection],
        route: R,
        atomicity: Atomicity,
    ) -> Result<ImportReport<I>, ImportError<Self::Error, I>>
    where
        I: Id,
        R: Fn(&[u8]) -> I + Send,
    {
        match atomicity {
            Atomicity::PerStream => import_per_stream(self, sections, route).await,
            Atomicity::WholeChunk => import_whole_chunk(self, sections, route).await,
        }
    }
}
```

Add a minimal `import_whole_chunk` stub so the module compiles this task (Task 5
replaces its body fully). The stub must still type-check:

```rust
/// WholeChunk import — implemented in Task 5.
async fn import_whole_chunk<S, I, R>(
    store: &S,
    sections: &[StreamSection],
    route: R,
) -> Result<ImportReport<I>, ImportError<S::Error, I>>
where
    S: RawEventStore + AtomicAppend,
    I: Id,
    R: Fn(&[u8]) -> I + Send,
{
    // Placeholder body — replaced in Task 5. Compiles, never used by Task 4 tests.
    let _ = (store, sections, &route);
    Ok(ImportReport::new(Vec::new()))
}
```

(Plan note: a stub that returns a wrong result is acceptable ONLY because no
Task-4 test calls WholeChunk. Task 5 begins by replacing this body and adding
its tests, so the stub never ships in a tested-but-wrong state.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing per_stream_`
Expected: PASS (all PerStream tests).

- [ ] **Step 5: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
feat(store): blanket EventImporter — PerStream (picky, halt-not-skip)

PerStream maps the planner's run + halt + append result to a StreamOutcome per
section: Conflict ⇒ Mismatch{reached:None, got:first} (the picky/idempotency
reject), Gap ⇒ Mismatch{Some(last), got}, Corrupt ⇒ Corrupt{reached}. WholeChunk
is a compiling stub, filled next.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Blanket `EventImporter` — WholeChunk

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Write the failing WholeChunk + atomicity tests**

Add to the `import.rs` test module:

```rust
    #[tokio::test]
    async fn whole_chunk_clean_commits_all_complete() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![
            section("a", vec![evt(1), evt(2)]),
            section("b", vec![evt(1)]),
        ];
        let report = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect("import ok");
        assert!(report.all_complete());
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2]);
        assert_eq!(versions(&store, &tid("b")).await, vec![1]);
    }

    #[tokio::test]
    async fn whole_chunk_corrupt_block_aborts_nothing_lands() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![
            section("a", vec![evt(1), evt(2)]),                 // would be fine
            section("b", vec![evt(1), ImportBlock::Corrupt]),   // bad block
        ];
        let err = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect_err("aborts");
        match err {
            ImportError::Aborted { stream, reason } => {
                assert_eq!(stream, tid("b"));
                assert_eq!(reason, AbortReason::Corrupt);
            }
            other => panic!("expected Aborted, got {other:?}"),
        }
        // Atomicity: NOTHING landed — even the clean section "a" must be empty.
        assert_eq!(versions(&store, &tid("a")).await, Vec::<u64>::new());
        assert_eq!(versions(&store, &tid("b")).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn whole_chunk_internal_gap_aborts_mismatch() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![evt(1), evt(2), evt(4)])];
        let err = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect_err("aborts");
        match err {
            ImportError::Aborted { stream, reason } => {
                assert_eq!(stream, tid("a"));
                assert_eq!(reason, AbortReason::Mismatch { expected: v(3), got: v(4) });
            }
            other => panic!("expected Aborted, got {other:?}"),
        }
        assert_eq!(versions(&store, &tid("a")).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn whole_chunk_head_conflict_aborts_mismatch() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        // "a" already at v2 → a fresh-stream section conflicts at commit.
        for n in 1..=2 {
            store
                .append(&tid("a"), Version::new(n - 1), &[pending(n, b"seed")])
                .await
                .expect("seed");
        }
        let sections = vec![section("a", vec![evt(1), evt(2)])];
        let err = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect_err("aborts");
        match err {
            ImportError::Aborted { stream, reason } => {
                assert_eq!(stream, tid("a"));
                // store head 2 → wanted v3 next; offered v1.
                assert_eq!(reason, AbortReason::Mismatch { expected: v(3), got: v(1) });
            }
            other => panic!("expected Aborted, got {other:?}"),
        }
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2], "unchanged");
    }

    #[tokio::test]
    async fn whole_chunk_first_block_corrupt_aborts() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![ImportBlock::Corrupt])];
        let err = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect_err("aborts");
        assert!(matches!(
            err,
            ImportError::Aborted { ref stream, reason: AbortReason::Corrupt } if *stream == tid("a")
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing whole_chunk_ -- --nocapture`
Expected: FAIL — the stub returns an empty report / wrong result, so assertions fail (e.g. `whole_chunk_clean_commits_all_complete` sees no events).

- [ ] **Step 3: Replace the `import_whole_chunk` stub with the real body**

In `crates/nexus-store/src/import.rs`, replace the placeholder `import_whole_chunk`
body with:

```rust
/// WholeChunk import: all-or-nothing across every section. Any halt (corrupt
/// block or internal gap) or head conflict aborts the whole chunk — nothing
/// lands. First offender (section order, then block order) wins.
async fn import_whole_chunk<S, I, R>(
    store: &S,
    sections: &[StreamSection],
    route: R,
) -> Result<ImportReport<I>, ImportError<S::Error, I>>
where
    S: RawEventStore + AtomicAppend,
    I: Id,
    R: Fn(&[u8]) -> I + Send,
{
    // Phase 1 — plan every section purely. Any halt is a hard abort here.
    let mut writes: Vec<PlannedAppend<I>> = Vec::with_capacity(sections.len());
    let mut lasts: Vec<Version> = Vec::with_capacity(sections.len());
    for section in sections {
        let target = route(section.origin.as_ref());
        let plan = match plan_section(section) {
            Ok(plan) => plan,
            Err(PlanError::VersionOverflow) => return Err(ImportError::VersionOverflow),
        };
        match plan {
            SectionPlan::Empty => continue,
            SectionPlan::FirstCorrupt => {
                return Err(ImportError::Aborted {
                    stream: target,
                    reason: AbortReason::Corrupt,
                });
            }
            SectionPlan::Run {
                expected_version,
                events,
                last,
                halt,
                ..
            } => {
                match halt {
                    Halt::Complete => {}
                    Halt::Corrupt => {
                        return Err(ImportError::Aborted {
                            stream: target,
                            reason: AbortReason::Corrupt,
                        });
                    }
                    Halt::Gap { got } => {
                        let expected = last.next().ok_or(ImportError::VersionOverflow)?;
                        return Err(ImportError::Aborted {
                            stream: target,
                            reason: AbortReason::Mismatch { expected, got },
                        });
                    }
                }
                lasts.push(last);
                writes.push(PlannedAppend {
                    target,
                    expected_version,
                    events,
                });
            }
        }
    }

    // Phase 2 — commit every clean run in one transaction.
    match store.atomic_append_many(&writes).await {
        Ok(()) => {
            let reports = writes
                .into_iter()
                .zip(lasts)
                .map(|(write, last)| StreamReport {
                    stream: write.target,
                    outcome: StreamOutcome::Complete { version: last },
                })
                .collect();
            Ok(ImportReport::new(reports))
        }
        Err(AtomicAppendError::Conflict { index, actual }) => {
            // `index` < writes.len() by the primitive's contract; every run is
            // non-empty by the planner's contract.
            let got = writes[index].events[0].version();
            let stream = writes[index].target.clone();
            let expected = match actual {
                Some(head) => head.next().ok_or(ImportError::VersionOverflow)?,
                None => Version::INITIAL,
            };
            Err(ImportError::Aborted {
                stream,
                reason: AbortReason::Mismatch { expected, got },
            })
        }
        Err(AtomicAppendError::Store(error)) => Err(ImportError::Store(error)),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing whole_chunk_`
Expected: PASS (all WholeChunk tests). Also re-run PerStream to ensure no regression:
`nix develop -c cargo test -p nexus-store --features export,import,testing per_stream_`

- [ ] **Step 5: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
feat(store): blanket EventImporter — WholeChunk (all-or-nothing via AtomicAppend)

WholeChunk plans every section, treats any halt (corrupt block or internal gap)
or head conflict as Err(Aborted{stream, reason}) with nothing landed, and on a
fully-clean chunk commits all runs in one atomic_append_many transaction.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Re-exports + round-trip + idempotency

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Add the public re-exports**

In `crates/nexus-store/src/lib.rs`, replace the import re-export block:

```rust
#[cfg(feature = "import")]
pub use import::{
    AbortReason, Atomicity, EventImporter, ImportError, ImportReport, StreamOutcome, StreamReport,
};
```

with (adds the user/box-facing `ImportBlock` + `StreamSection`; `AtomicAppend`,
`PlannedAppend`, `AtomicAppendError` stay at `nexus_store::import::*`, matching
the adapter-facing `RawSubscription` convention of not re-exporting at the root):

```rust
#[cfg(feature = "import")]
pub use import::{
    AbortReason, Atomicity, EventImporter, ImportBlock, ImportError, ImportReport, StreamOutcome,
    StreamReport, StreamSection,
};
```

- [ ] **Step 2: Write the failing round-trip + idempotency tests**

Add to the `import.rs` test module. These wire Card-1 export → Card-2 import:

```rust
    use crate::export::{EventExporter, StreamLister};

    async fn export_section(store: &InMemoryStore, origin: &str) -> StreamSection {
        let blocks = store
            .export_stream(&tid(origin), Version::INITIAL)
            .await
            .expect("export opens")
            .map(|r| ImportBlock::Event(r.expect("no read error")))
            .collect::<Vec<_>>()
            .await;
        section(origin, blocks)
    }

    #[tokio::test]
    async fn export_then_import_round_trips_byte_equal_modulo_global_seq() {
        use crate::import::EventImporter;
        // Source store with two streams.
        let source = InMemoryStore::new();
        for n in 1..=3 {
            source
                .append(&tid("acct-1"), Version::new(n - 1), &[pending(n, b"a")])
                .await
                .expect("seed a");
        }
        source
            .append(&tid("acct-2"), None, &[pending(1, b"b")])
            .await
            .expect("seed b");

        let sections = vec![
            export_section(&source, "acct-1").await,
            export_section(&source, "acct-2").await,
        ];

        // Import into a FRESH store, identity routing.
        let target = InMemoryStore::new();
        let report = target
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");
        assert!(report.all_complete());

        // Byte-equal modulo restamped global_seq: compare version/type/payload/
        // metadata/schema for every event of every stream.
        for origin in ["acct-1", "acct-2"] {
            let src: Vec<PersistedEnvelope> = source
                .read_stream(&tid(origin), Version::INITIAL)
                .await
                .expect("read")
                .map(|r| r.expect("ok"))
                .collect()
                .await;
            let dst: Vec<PersistedEnvelope> = target
                .read_stream(&tid(origin), Version::INITIAL)
                .await
                .expect("read")
                .map(|r| r.expect("ok"))
                .collect()
                .await;
            assert_eq!(src.len(), dst.len(), "{origin} length");
            for (s, d) in src.iter().zip(dst.iter()) {
                assert_eq!(s.version(), d.version(), "{origin} version");
                assert_eq!(s.event_type(), d.event_type(), "{origin} type");
                assert_eq!(s.payload(), d.payload(), "{origin} payload");
                assert_eq!(s.metadata(), d.metadata(), "{origin} metadata");
                assert_eq!(s.schema_version(), d.schema_version(), "{origin} schema");
            }
        }
    }

    #[tokio::test]
    async fn reimport_same_chunk_is_idempotent_per_stream() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![evt(1), evt(2), evt(3)])];

        let first = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");
        assert!(first.all_complete());

        // Re-import the same chunk: refused by the picky check, nothing duplicated.
        let second = store
            .import(&sections, identity_route, Atomicity::PerStream)
            .await
            .expect("import ok");
        assert_eq!(
            second.streams()[0].outcome,
            StreamOutcome::Mismatch { reached: None, got: v(1) }
        );
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2, 3], "no dupes");
    }

    #[tokio::test]
    async fn reimport_same_chunk_whole_chunk_aborts() {
        use crate::import::EventImporter;
        let store = InMemoryStore::new();
        let sections = vec![section("a", vec![evt(1), evt(2)])];
        store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect("first import ok");
        let err = store
            .import(&sections, identity_route, Atomicity::WholeChunk)
            .await
            .expect_err("re-import aborts");
        assert!(matches!(err, ImportError::Aborted { .. }));
        assert_eq!(versions(&store, &tid("a")).await, vec![1, 2], "no dupes");
    }
```

`StreamLister` is imported only to keep the export-side path available; if
clippy flags it unused, drop it from the `use` (only `EventExporter` is needed
for `export_stream`).

- [ ] **Step 3: Run the tests to verify they fail (then pass)**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing round_trips reimport -- --nocapture`
Expected: PASS once re-exports compile (the behavior is already implemented in
Tasks 4–5; this task adds coverage of the export→import seam + idempotency).
If a `use` path fails to resolve, fix the re-export from Step 1.

- [ ] **Step 4: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/lib.rs crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
feat(store): export→import round-trip + idempotent re-import; public re-exports

Re-exports StreamSection/ImportBlock at the crate root (AtomicAppend stays
adapter-facing under import::*). Tests prove a Card-1 export re-imports
byte-equal modulo restamped global_seq, and re-importing a chunk is refused by
the picky check with nothing duplicated (both atomicity modes).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Linearizability — concurrent import vs writer

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Write the concurrency test**

Add to the `import.rs` test module (needs `use std::sync::Arc;` and
`use tokio::sync::Barrier;` in the test module's imports):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn per_stream_import_concurrent_with_writer_surfaces_conflict_no_torn_stream() {
        use crate::import::EventImporter;
        // Two actors race to populate the SAME fresh target stream from v1:
        // a direct writer (append v1..=N) and an importer (a v1..=N section).
        // Exactly one wins the v1 slot; the other must surface a Mismatch (a
        // conflict, never a silent retry — CLAUDE rule 5). The stream must end
        // a gapless prefix from v1, never torn.
        let store = Arc::new(InMemoryStore::new());
        let n = 50u64;
        let barrier = Arc::new(Barrier::new(2));

        let writer_store = Arc::clone(&store);
        let writer_barrier = Arc::clone(&barrier);
        let writer = tokio::spawn(async move {
            writer_barrier.wait().await;
            for v in 1..=n {
                // Conflicts are expected once the importer wins; ignore them.
                let _ = writer_store
                    .append(&tid("race"), Version::new(v - 1), &[pending(v, b"w")])
                    .await;
            }
        });

        let importer_store = Arc::clone(&store);
        let importer_barrier = Arc::clone(&barrier);
        let importer = tokio::spawn(async move {
            importer_barrier.wait().await;
            let blocks: Vec<ImportBlock> = (1..=n).map(evt).collect();
            let sections = vec![section("race", blocks)];
            importer_store
                .import(&sections, identity_route, Atomicity::PerStream)
                .await
                .expect("import returns Ok (conflicts are per-stream outcomes)")
        });

        writer.await.expect("writer task");
        let report = importer.await.expect("importer task");

        // The importer's outcome is either Complete (it won v1) or Mismatch
        // (the writer won) — never a partial/torn application.
        let outcome = report.streams().first().map(|s| s.outcome);
        match outcome {
            Some(StreamOutcome::Complete { .. }) | Some(StreamOutcome::Mismatch { reached: None, .. }) => {}
            other => panic!("unexpected importer outcome: {other:?}"),
        }

        // Whatever the interleaving, the final stream is a gapless prefix from 1.
        let final_versions = versions(&store, &tid("race")).await;
        for (expected, got) in (1u64..).zip(final_versions.iter()) {
            assert_eq!(*got, expected, "stream must be a gapless prefix from 1");
        }
        assert!(!final_versions.is_empty(), "some events landed");
    }
```

- [ ] **Step 2: Run it to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing per_stream_import_concurrent`
Expected: PASS (the behavior already holds; this proves it under real overlap).
Run a few times to shake out flakiness:
`for i in 1 2 3; do nix develop -c cargo test -p nexus-store --features export,import,testing per_stream_import_concurrent || break; done`

- [ ] **Step 3: Verify clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Expected: no warnings.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
test(store): linearizability — concurrent import vs writer on one stream

A direct writer and an importer race the same fresh stream from v1; exactly one
wins, the loser surfaces a Mismatch (conflict surfaced, never retried — CLAUDE
rule 5), and the final stream is always a gapless prefix from v1.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Property-based state machine

**Files:**
- Modify: `crates/nexus-store/src/import.rs`

- [ ] **Step 1: Write the state-machine property test**

Add to the `import.rs` test module (needs `use proptest::prelude::*;`). The
model is per-target-stream expected-next-version; the test drives real imports
and checks the report against an oracle and asserts no stream ever has a gap.

```rust
    // Model-based: a sequence of single-stream PerStream imports against one
    // target. The model tracks the stream's next-expected version; each import
    // offers a first version + a contiguous block count + an optional trailing
    // gap. Invariants: the reported outcome matches the model's picky rule, the
    // stream never develops a gap, and what landed matches the report.
    #[derive(Debug, Clone)]
    enum Cmd {
        // Offer `count` contiguous events starting at `first`.
        Import { first: u64, count: u64 },
    }

    fn cmd_strategy() -> impl Strategy<Value = Cmd> {
        // Boundary-inclusive: first ∈ {1,2,3}, count ∈ {1,2,3}.
        (prop_oneof![Just(1u64), Just(2u64), Just(3u64)], 1u64..=3)
            .prop_map(|(first, count)| Cmd::Import { first, count })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn state_machine_per_stream_never_gaps_and_report_matches_model(
            cmds in proptest::collection::vec(cmd_strategy(), 1..12),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let store = InMemoryStore::new();
                let id = tid("m");
                // Model: next expected version (1 = fresh).
                let mut next: u64 = 1;

                for cmd in cmds {
                    let Cmd::Import { first, count } = cmd;
                    let blocks: Vec<ImportBlock> =
                        (first..first + count).map(evt).collect();
                    let sections = vec![section("m", blocks)];
                    let report = {
                        use crate::import::EventImporter;
                        store
                            .import(&sections, identity_route, Atomicity::PerStream)
                            .await
                            .expect("import ok")
                    };
                    let outcome = report.streams()[0].outcome;

                    if first == next {
                        // Picky check passes: the whole contiguous run applies.
                        let landed_last = first + count - 1;
                        prop_assert_eq!(
                            outcome,
                            StreamOutcome::Complete { version: v(landed_last) }
                        );
                        next = landed_last + 1;
                    } else {
                        // Picky reject: nothing applied, model unchanged.
                        prop_assert_eq!(
                            outcome,
                            StreamOutcome::Mismatch { reached: None, got: v(first) }
                        );
                    }

                    // Invariant: the stream is ALWAYS a gapless prefix 1..next-1.
                    let got = versions(&store, &id).await;
                    let expected: Vec<u64> = (1..next).collect();
                    prop_assert_eq!(got, expected);
                }
                Ok(())
            })?;
        }
    }
```

- [ ] **Step 2: Run it to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features export,import,testing state_machine_per_stream`
Expected: PASS (64 cases). If a case fails, proptest shrinks it — fix the bug it
found (do not weaken the test).

- [ ] **Step 3: Verify clippy + full local suite + commit**

Run: `nix develop -c cargo clippy -p nexus-store --features export,import,testing --all-targets`
Run: `nix develop -c cargo nextest run -p nexus-store --features export,import,testing`
Expected: no warnings; all tests pass.

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/import.rs
git commit -m "$(cat <<'EOF'
test(store): proptest state machine for PerStream import (no gaps; report=model)

Drives a sequence of single-stream imports against an oracle that tracks the
next-expected version: the picky rule, the per-stream outcome, and the no-gap
invariant must all match what actually landed, for boundary-inclusive first/
count ranges.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Final verification

- [ ] **Step 1: Confirm the feature compiles cleanly in isolation**

Run: `nix develop -c cargo clippy -p nexus-store --features import,testing --all-targets`
(import without export — ensures no accidental cross-feature dependency)
Expected: no warnings.

- [ ] **Step 2: Confirm default-feature build is unaffected**

Run: `nix develop -c cargo clippy -p nexus-store --all-targets`
Expected: no warnings (import code is fully gated).

- [ ] **Step 3: Let the pre-commit hook run the gate on the final state**

There is nothing left to commit if Tasks 1–8 each committed. If any working-tree
changes remain (e.g. fmt), stage and commit them — the pre-commit hook runs
`nix flake check` for you. **Do NOT run `nix flake check` by hand.**

- [ ] **Step 4: Open the PR**

```bash
git push -u origin feat/export-import-traits
gh pr create --title "feat(store): export/import card 2 — EventImporter (picky, halt-not-skip)" \
  --body "$(cat <<'EOF'
Implements Card 2 of #145: the EventImporter — per-stream sections, picky-via-append, halt-not-skip, per-stream report, both Atomicity modes.

- One `EventImporter: RawEventStore + AtomicAppend` trait (blanket impl); new `AtomicAppend` cross-stream transaction primitive (InMemoryStore impl via single mutex).
- Pure per-section planner (longest contiguous run + halt reason) shared by both modes.
- PerStream → per-section `append`; WholeChunk → one `atomic_append_many`, Err(Aborted) on first bad block, nothing landed.
- Version-preserving; checked arithmetic ⇒ VersionOverflow (not a conflict).
- Tests: 4 cross-cutting categories (incl. export→import round-trip, idempotent re-import, mid-section corrupt, WholeChunk-leaves-store-unchanged, concurrent import vs writer) + proptest state machine.

Design: `docs/plans/2026-06-21-export-import-card2-import-design.md`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Use the `joeldsouzax` gh account (per repo policy).

---

## Deviation Log

Export/import is a multi-card feature; per project policy, log any divergence
from this plan here as it happens (reason + impact), so Card 3 inherits accurate
context. Do **not** log routine clippy/fmt micro-adjustments — strict clippy is
the project default, not a deviation.

| Date | Task | Deviation | Reason | Impact |
|------|------|-----------|--------|--------|
| —    | —    | (none yet)|        |        |

---

## Self-Review

**Spec coverage:**
- §2 input shape → Task 2 (`StreamSection`/`ImportBlock`), Task 4/5 signature use. ✓
- §3 trait architecture (`EventImporter: RawEventStore + AtomicAppend`, blanket) → Task 1 (`AtomicAppend`), Task 2 (supertrait), Task 4 (blanket impl). ✓
- §4 per-section planner → Task 3. ✓
- §4a PerStream mapping → Task 4. ✓
- §4b WholeChunk mapping → Task 5. ✓
- §5 version safety → Task 3 (`checked_sub`/`next`), Task 5 (conflict expected). ✓
- §6 `from_persisted` → Task 1. ✓
- §7 files touched → all four files appear across tasks. ✓
- §8 tests: cat 1 (Task 4/6), cat 2 (Task 4), cat 3 (Task 4/5), cat 4 (Task 7), atomicity (Task 5), state machine (Task 8). ✓

**Placeholder scan:** the only intentional stub is `import_whole_chunk` in Task 4,
explicitly replaced in Task 5 Step 3 before any WholeChunk test exists — flagged
in-plan, never shipped tested-but-wrong. No other TODO/TBD.

**Type consistency:** `plan_section -> Result<SectionPlan, PlanError>`;
`SectionPlan::{Empty, FirstCorrupt, Run{first, expected_version, events, last, halt}}`;
`Halt::{Complete, Corrupt, Gap{got}}`; `PlannedAppend{target, expected_version, events}`;
`AtomicAppendError::{Conflict{index, actual}, Store}`; `EventImporter::import(sections, route, atomicity)`.
Names are consistent across Tasks 1–8.
