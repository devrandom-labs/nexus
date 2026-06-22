# Export/Import — Card 2: the `EventImporter` implementation

Issue: devrandom-labs/nexus#145 · Branch: `feat/export-import-traits` · Card 1: `66040be`

Card 2 turns the frozen Card-0 import *contract* (`StreamOutcome`,
`StreamReport`, `ImportReport`, `AbortReason`, `ImportError`, `Atomicity`) into
a working importer. It is the mirror of Card 1's export (raw, box-agnostic,
generic over the store). Card 3 (the CBOR box) later produces the input this
card consumes.

## 1. What import does

Import takes already-decoded, per-stream **sections** (the box's job is bytes →
sections) and places each section's events onto a caller-routed target stream.
It adds three things the store's `append` does not: routing, a halt-on-trouble
rule, and a per-stream report. It is **picky** (a stream's first version must be
exactly the target's next expected version), **halt-not-skip** (a gap or corrupt
block stops *its* stream and holds back everything after it — never punches a
gap), and **version-preserving** (imported events keep their origin versions;
each origin maps to its own target stream).

Idempotency is a free side effect of the picky check: re-importing
already-present events conflicts on the first version and is refused — no dedup
machinery, nothing duplicated.

## 2. Input shape (resolved)

The Card-0 provisional `events: &[PersistedEnvelope]` cannot route — a
`PersistedEnvelope` carries no stream id (export does no rewrite). Replaced by
per-stream sections:

```rust
/// One origin stream's events, as decoded by the box.
pub struct StreamSection {
    /// Origin stream id, recorded once by the box (never per event).
    pub origin: Bytes,
    /// The section's blocks, in version order as the box laid them down.
    pub blocks: Vec<ImportBlock>,
}

/// One block within a section.
pub enum ImportBlock {
    /// A block whose checksum passed and decoded to an event.
    Event(PersistedEnvelope),
    /// A block whose per-block checksum FAILED. Carries nothing: a failed
    /// checksum means the decoded header (incl. its version) cannot be
    /// trusted — which is exactly why `StreamOutcome::Corrupt` omits the
    /// version.
    Corrupt,
}
```

`import` signature (replaces the provisional one; the frozen *data types* are
unchanged):

```rust
fn import<I, R>(
    &self,
    sections: &[StreamSection],
    route: R,
    atomicity: Atomicity,
) -> impl Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
where
    I: Id,
    R: Fn(&[u8]) -> I + Send;
```

`route(&section.origin) -> I` is where origin → target namespacing happens
(e.g. `task-123` → `phone:task-123`). Import owns no naming policy; the target
id is echoed verbatim into each `StreamReport`. Sections are borrowed; the
importer rebuilds owned `PendingEnvelope`s from them (see §6).

## 3. Trait architecture (resolved: single trait + `AtomicAppend` supertrait)

`RawEventStore::append` is per-stream; it exposes no cross-stream transaction.
WholeChunk (all-or-nothing across many streams) needs one. Rather than fake it
(CLAUDE rule 1), import requires a transactional capability as a supertrait:

```rust
/// Adapter capability: commit several per-stream runs in ONE atomic
/// transaction. Either every run lands or none do.
pub trait AtomicAppend: RawEventStore {
    fn atomic_append_many<I: Id>(
        &self,
        writes: &[PlannedAppend<I>],
    ) -> impl Future<Output = Result<(), AtomicAppendError<Self::Error>>> + Send;
}

/// One planned per-stream write: target, the head we expect, the run to append.
pub struct PlannedAppend<I> {
    pub target: I,
    pub expected_version: Option<Version>, // None = fresh stream
    pub events: Vec<PendingEnvelope>,      // contiguous, version-preserving
}

#[derive(thiserror::Error)]
pub enum AtomicAppendError<E> {
    /// Write `index`'s expected head didn't match the actual head (which is
    /// `actual`). Whole transaction rolled back.
    Conflict { index: usize, actual: Option<Version> },
    Store(#[source] E),
}

/// Import is available for any store that is both a raw store and atomically
/// appendable. All routing/halt/report logic lives in this ONE blanket impl.
impl<S: RawEventStore + AtomicAppend> EventImporter for S { /* ... */ }
```

- **PerStream** uses `RawEventStore::append` once per section — each section its
  own transaction, a bad block stops only its stream, others commit, returns
  `Ok(ImportReport)`.
- **WholeChunk** uses `AtomicAppend::atomic_append_many` once for all sections —
  any bad block returns `Err(Aborted)` and nothing lands.

`InMemoryStore` implements `AtomicAppend` via its single `streams` mutex (a
validate-all-then-apply-all critical section is genuinely atomic). Fjall and
postgres implement it later via a cross-partition `write_tx` / `BEGIN..COMMIT`.
Trade-off accepted: a store with no transaction cannot be an `EventImporter`
even for PerStream — every real adapter has transactions, so this is fine.

`AtomicAppend`, `PlannedAppend`, `AtomicAppendError`, `StreamSection`,
`ImportBlock` live in `import.rs` (feature `import`). The `InMemoryStore` impl
lives in `testing.rs` under `#[cfg(feature = "import")]`.

## 4. Per-section algorithm (the pure core, shared by both modes)

For each section, with no store access:

1. If `blocks` is empty → no work, no report entry (the box never emits empty
   sections; handled defensively).
2. Inspect `blocks[0]`:
   - `Corrupt` → the section is corrupt from the start, nothing to append.
   - `Event(e0)` → `first = e0.version()`; `expected_version = first - 1`
     (`checked`; `first == 1` → `None` = fresh stream).
3. Walk `blocks[1..]`, accumulating the **longest contiguous run**:
   - `Corrupt` → halt (reason: corrupt).
   - `Event(e)` where `e.version() == prev + 1` (checked; overflow →
     `ImportError::VersionOverflow`) → extend the run.
   - `Event(e)` where `e.version() != prev + 1` → halt (reason: gap, `got =
     e.version()`).
4. The run + halt reason are the plan. Convert each run event to a
   `PendingEnvelope` (§6).

The plan is store-independent; both atomicity modes build it identically. Only
the head-check and the write differ.

### 4a. PerStream outcome mapping

Append the run via `RawEventStore::append(target, expected_version, run)`:

| append result | first-block was | outcome |
|---|---|---|
| `Ok` | — run consumed all blocks | `Complete { version: last }` |
| `Ok` | — halted at corrupt | `Corrupt { reached: Some(last) }` |
| `Ok` | — halted at gap (`got`) | `Mismatch { reached: Some(last), got }` |
| (n/a) | first block `Corrupt` | `Corrupt { reached: None }` |
| `Conflict` | any | `Mismatch { reached: None, got: first }` |
| `Store(e)` | any | propagate `Err(ImportError::Store(e))` |

Notes:
- `Conflict` (store head ≠ `first - 1`) is the picky rejection: stale overlap,
  forward gap, or already-present (idempotent) re-import. `append` is atomic, so
  nothing landed → `reached: None`. `got = first` (the rejected version).
- `Corrupt` omits a version; `Mismatch` carries `got` — exactly the frozen-type
  asymmetry.
- A `Store` error mid-import surfaces as `Err`; sections committed before it
  stay committed (PerStream promises no cross-stream rollback). Documented.

### 4b. WholeChunk outcome mapping

WholeChunk is all-or-nothing, so a *halt is an abort* — any corrupt block or any
internal gap fails the whole chunk. First offender (section order, then block
order) wins:

1. Plan every section (§4). If any plan halts:
   - halted/started at corrupt → `Err(Aborted { stream: route(origin), reason:
     AbortReason::Corrupt })`.
   - halted at gap → `Err(Aborted { stream, reason: Mismatch { expected: prev+1,
     got } })`.
   - `VersionOverflow` building a run → `Err(ImportError::VersionOverflow)`.
2. All sections clean & fully contiguous → collect `PlannedAppend`s and commit
   via `atomic_append_many`:
   - `Ok` → `Ok(ImportReport)` with every stream `Complete { version: last }`.
   - `Conflict { index, actual }` → `Err(Aborted { stream:
     writes[index].target, reason: Mismatch { expected: actual.next()
     /* or INITIAL if empty */, got: first } })`. Nothing landed.
   - degenerate: `actual == u64::MAX` (next overflows) →
     `Err(ImportError::VersionOverflow)`.
   - `Store(e)` → `Err(ImportError::Store(e))`.

`ImportError::Malformed` is never produced here — chunk-header parsing is the
box's job (Card 3); it lives in the shared error enum for that layer.

## 5. Version safety (CLAUDE rule 2)

- `expected_version = first - 1`: `first.as_u64().checked_sub(1).and_then(
  Version::new)` → `None` for `first == 1` (fresh stream) — no bare arithmetic.
- next-expected within a run: `prev.next()` (checked) → `None` →
  `ImportError::VersionOverflow` (not a `Conflict`, not retryable).
- WholeChunk conflict expected-next: `actual.next()` checked; overflow →
  `VersionOverflow`.

## 6. `PersistedEnvelope` → `PendingEnvelope` (infallible, zero re-validation)

The read-path envelope must become a write-path one, dropping `global_seq` (the
store restamps a fresh one on append). A fresh fallible builder
(`pending_envelope(v).event_type_bytes(..)?.payload(..)?…`) would re-validate
already-validated bytes and introduce an "impossible" error with no home in the
frozen `ImportError`. Instead, reuse the validated value newtypes:

```rust
// pub(crate), envelope.rs — both crates that need it are in nexus-store.
impl PendingEnvelope {
    pub(crate) fn from_persisted(p: &PersistedEnvelope) -> Self {
        // version, event_type_value(), schema_version_value(),
        // payload_value(), metadata_value() are all already validated and
        // Arc-shared. No copy, no re-validation, infallible. global_seq dropped.
    }
}
```

This keeps `ImportError` frozen and respects "borrow before own / no gratuitous
allocations" (each field is one Arc share).

## 7. Files touched (all in `nexus-store`)

- `import.rs` — add `StreamSection`, `ImportBlock`, `AtomicAppend`,
  `PlannedAppend`, `AtomicAppendError`; replace the provisional `EventImporter`
  signature; add the blanket `impl<S: RawEventStore + AtomicAppend>
  EventImporter`; the pure per-section planner; outcome mapping. Delete the
  `NoopStore` scaffold (it manually impl'd `EventImporter`, which now collides
  with the blanket impl). **Keep every frozen-type test.**
- `testing.rs` — `impl AtomicAppend for InMemoryStore` (single-mutex,
  validate-all-then-apply-all), `#[cfg(feature = "import")]`.
- `envelope.rs` — `pub(crate) PendingEnvelope::from_persisted`.
- `lib.rs` — re-export the new public items under `#[cfg(feature = "import")]`.
- New `tests/` integration file for the import behavioral + state-machine
  suite (see §8).

## 8. Test plan (TDD; 4 cross-cutting categories FIRST, then state machine)

**1 — Sequence / protocol.** Clean multi-stream chunk → all `Complete`. Card-1
export → import into a fresh store → streams byte-equal modulo restamped
`global_seq` (compare version/event_type/payload/metadata/schema). Re-import the
same chunk → idempotent: PerStream all `Mismatch { reached: None, got: v1 }`,
WholeChunk `Err(Aborted)`, store row count unchanged either way.

**2 — Lifecycle.** Empty chunk → empty report, `all_complete()` vacuously true.
Partial overlap (target at v3, section offers v2..v5) → picky reject (`Mismatch{
None, got: v2 }`), nothing applied. Internal gap (v3,v4,v6) → applies v3,v4,
halts → `Mismatch { reached: Some(v4), got: v6 }`, v6 held.

**3 — Defensive boundary.** Corrupt block mid-section (v3,v4,CORRUPT,v6) →
`Corrupt { reached: Some(v4) }`, good prefix applied, later blocks held. First
block corrupt → `Corrupt { reached: None }`. Version overflow near `u64::MAX`
(run forces `prev.next()` overflow) → `ImportError::VersionOverflow`. Mismatched
routing (two origins → same target) → second section conflicts.

**4 — Linearizability / isolation.** PerStream import concurrent
(`tokio::spawn` + `Barrier`) with another writer on an overlapping target →
no torn stream, every read a gapless prefix, the loser surfaces a `Mismatch`
(conflict surfaced, never retried internally — CLAUDE rule 5).

**Atomicity.** WholeChunk with one bad block (corrupt or gap) anywhere →
`Err(Aborted { stream, reason })` AND the store is byte-for-byte unchanged
(assert every target stream's read is empty/unchanged). PerStream with one bad
block → `Ok(report)` where clean streams are `Complete` and the bad one halted.

**State machine (proptest, the headline methodology).** Model = per-target-stream
expected next version. Commands: append-locally, import-section (varied first
version, internal gaps, corrupt blocks, both atomicity modes). Invariants: no
gaps ever created in any stream; picky refusal matches the model; the report
matches what actually landed (read back and compare); idempotent re-import is a
no-op. Strategies include boundary versions (1, 2, MAX-1, MAX) per CLAUDE rule.

## 9. Constraints

- Feature `import`; behavioral tests need `testing` (InMemoryStore) — the self
  dev-dep already enables `testing,export,import`.
- Strict clippy (pedantic + nursery, no `as`, no `unwrap`/`expect` outside
  tests, thiserror only, all `use` at top, named concrete types — no
  `BoxStream`/dyn on event paths, `std::thread::scope` not `spawn`).
- `git add` new/changed files before any commit (pre-commit hook runs
  `nix flake check`, which ignores untracked files). `cargo fmt --all` before
  staging. Do NOT pre-run the gate.
- Conventional commit `feat(store): …`, `Co-Authored-By: Claude Opus 4.8 (1M
  context) <noreply@anthropic.com>`.
