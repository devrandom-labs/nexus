# nexus-fjall Craftsmanship Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** De-bloat and elegantly re-factor `nexus-fjall` to the `nexus-postgres`
craftsmanship bar — kill gratuitous types, unify the duplicated write path behind
one pure planner, fix visibility leaks, relocate inline tests — behaviour-preserving.

**Architecture:** Four ordered tasks, each a gate-green commit. Pure simplifications
first (they shrink surface with the existing inline tests as safety net), then the
structural write-path unification (guarded by those tests + the shared conformance
suites), then test relocation last (so tests protect the earlier refactors in place
before moving). The centrepiece is the postgres pure/IO seam: a DB-free `plan_run`
that both `append` and `atomic_append_many` become thin IO shells over.

**Tech Stack:** Rust 2024, fjall 3 (`bytes_1`), `nexus-store` traits, `thiserror`,
`futures`, `proptest`. Gate: `nix flake check` via pre-commit hook (do NOT run by
hand). Tooling via `nix develop -c cargo ...`.

**Findings reference:** `docs/plans/2026-07-01-fjall-craftsmanship-findings.md`.

**Sign-off scope:** full pure planner (①), kill `DecodedEvent` (②), `pub mod → mod`
(③), de-`pub` `wire_key` internals (④), relocate inline roundtrip tests (⑤).

---

## Task 1: Kill the `DecodedEvent` gratuitous repack (finding ②)

`wire_key::decode_event_value` copies `wire::decode_frame`'s `DecodedFrame`
field-for-field into a structurally-identical local `DecodedEvent`, whose only
consumer (`scan::build_envelope`) immediately destructures it back. Consume
`wire::decode_frame` directly; delete both `DecodedEvent` and `decode_event_value`.

**Files:**
- Modify: `crates/nexus-fjall/src/scan.rs` (imports, `build_envelope`, both `decode` impls)
- Modify: `crates/nexus-fjall/src/wire_key.rs:184-223` (delete `DecodedEvent` + `decode_event_value`)

- [ ] **Step 1: Point `build_envelope` at `wire::decode_frame` output.**

In `scan.rs`, change the import block (lines 15-18) to drop `DecodedEvent` and
`decode_event_value`, keeping the key codecs:

```rust
use crate::wire_key::{decode_event_key, decode_global_key, encode_event_key, encode_global_key};
use nexus_store::wire;
```

Change `build_envelope` (lines 57-81) to take `wire::DecodedFrame` and read its
`offsets` sub-struct:

```rust
fn build_envelope(
    bytes_value: Bytes,
    decoded: wire::DecodedFrame,
    raw_version: u64,
    stream_id: ErrorId,
) -> Result<PersistedEnvelope, FjallError> {
    let version = Version::new(raw_version).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(raw_version),
    })?;

    PersistedEnvelope::try_new(
        version,
        bytes_value,
        decoded.schema_version,
        decoded.offsets.event_type,
        decoded.offsets.payload,
        decoded.offsets.metadata,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id,
        version: raw_version,
        source,
    })
}
```

- [ ] **Step 2: Update both `ScanStrategy::decode` impls to call `wire::decode_frame`.**

In `StreamScan::decode` (scan.rs ~110) and `GlobalScan::decode` (scan.rs ~145),
replace the `decode_event_value(&bytes_value)` call with the wire decoder,
preserving the exact same `CorruptValue` mapping. For `StreamScan::decode`:

```rust
let bytes_value: Bytes = value.into();
let decoded = wire::decode_frame(bytes_value.as_ref()).map_err(|_| FjallError::CorruptValue {
    stream_id: self.label,
    version: Some(version),
})?;
build_envelope(bytes_value, decoded, version, self.label)
```

For `GlobalScan::decode` (uses `ErrorId::default()` and `version_raw`):

```rust
let bytes_value: Bytes = value.into();
let decoded = wire::decode_frame(bytes_value.as_ref()).map_err(|_| FjallError::CorruptValue {
    stream_id: ErrorId::default(),
    version: Some(version_raw),
})?;
let env = build_envelope(bytes_value, decoded, version_raw, ErrorId::default())?;
Ok((position, env))
```

- [ ] **Step 3: Delete `DecodedEvent` and `decode_event_value` from `wire_key.rs`.**

Remove `wire_key.rs` lines 184-223 (the `DecodedEvent` struct doc + def and the
`decode_event_value` fn). The `use nexus_store::wire;` import at line 1 stays (the
`EncodeError::Wire` variant still uses `wire::WireError`).

- [ ] **Step 4: Check any `wire_key` test referencing the deleted items.**

Run: `nix develop -c grep -rn 'DecodedEvent\|decode_event_value' crates/nexus-fjall/`
Expected: no matches. If a `#[cfg(test)]` test in `wire_key.rs` referenced them,
delete that test (it tested a repack that no longer exists; `wire`'s own tests +
fjall's roundtrip tests cover decode).

- [ ] **Step 5: Build + test locally (not the full gate).**

Run: `nix develop -c cargo test -p nexus-fjall --all-features`
Expected: PASS (all fjall unit + integration tests green).

- [ ] **Step 6: Commit.**

```bash
git add crates/nexus-fjall/src/scan.rs crates/nexus-fjall/src/wire_key.rs
git commit   # message below; pre-commit hook runs nix flake check
```

Message:
```
refactor(fjall): consume wire::DecodedFrame directly, drop DecodedEvent repack

DecodedEvent duplicated wire::DecodedFrame field-for-field and its only
consumer destructured it straight back into PersistedEnvelope::try_new.
build_envelope now takes wire::DecodedFrame; both ScanStrategy::decode impls
call wire::decode_frame. Behaviour-preserving (same CorruptValue mapping).

Relates to #270.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 2: Fix visibility — `pub mod → mod` (③) + de-`pub` `wire_key` internals (④)

Match the postgres module-visibility shape (`mod` + selective `pub use`), and drop
`pub` from three `wire_key` items with zero external references.

**Files:**
- Modify: `crates/nexus-fjall/src/lib.rs:63-72`
- Modify: `crates/nexus-fjall/src/wire_key.rs` (three items: `event_key_size`, `STREAM_VERSION_SIZE`, `GLOBAL_KEY_SIZE`)

- [ ] **Step 1: `pub mod → mod` in `lib.rs`.**

Change lines 63-72 so every module is private; the `pub use` re-exports (74-78)
are the sole public surface, exactly like `nexus-postgres/src/lib.rs`:

```rust
mod builder;
mod error;
mod global_seq;
mod partition;
mod scan;
#[cfg(feature = "snapshot")]
mod snapshot;
mod store;
mod subscription_id;
mod wire_key;

pub use builder::FjallStoreBuilder;
pub use error::FjallError;
pub use global_seq::GlobalSeq;
pub use partition::KeyspaceConfig;
pub use store::FjallStore;
```

- [ ] **Step 2: De-`pub` the three internal `wire_key` items.**

`event_key_size` (wire_key.rs:53), `STREAM_VERSION_SIZE` (:47), `GLOBAL_KEY_SIZE`
(:121) are referenced only inside `wire_key.rs`. Drop `pub`:

```rust
const STREAM_VERSION_SIZE: usize = 8;
...
const fn event_key_size(id_len: usize) -> usize { ... }
...
const GLOBAL_KEY_SIZE: usize = 16;
```

Note: `GLOBAL_KEY_SIZE` is the return-type array size of the `pub`
`encode_global_key(...) -> [u8; GLOBAL_KEY_SIZE]`. A private const in a public fn
signature is legal within a private mod; if clippy objects, inline the literal `16`
into the signature. Verify by building.

- [ ] **Step 3: Verify nothing outside `wire_key` used those items.**

Run: `nix develop -c cargo build -p nexus-fjall --all-features`
Expected: PASS. A failure here names an external use — if so, keep that specific
item `pub(crate)` (not `pub`) and note it.

- [ ] **Step 4: Test.**

Run: `nix develop -c cargo test -p nexus-fjall --all-features`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/nexus-fjall/src/lib.rs crates/nexus-fjall/src/wire_key.rs
git commit
```

Message:
```
refactor(fjall): mod + pub use over pub mod; de-pub wire_key internals

Match nexus-postgres module visibility: private modules with a selective
pub use surface (rule 4 — pub mod leaks internals). Drop pub from three
wire_key items (event_key_size, STREAM_VERSION_SIZE, GLOBAL_KEY_SIZE) that
have no references outside the module.

Relates to #270.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 3: Extract pure `plan_run`, unify both write paths (finding ①)

The centrepiece. `append` and `atomic_append_many` share one algorithm in two
error domains. Extract a DB-free `plan_run` producing the staged rows + new
counters; both public methods become thin IO shells mapping a neutral `PlanError`
into their own domain. TDD the pure core first (mirrors postgres `prepare_inserts`).

**Files:**
- Create: `crates/nexus-fjall/src/plan.rs` (pure planner + `PlanError` + `StagedRow` + `PlannedRun` + inline unit tests)
- Modify: `crates/nexus-fjall/src/lib.rs` (add `mod plan;`)
- Modify: `crates/nexus-fjall/src/store.rs` (`append` shell; `atomic_append_impl` shells over `plan_run`)
- Modify: `crates/nexus-fjall/src/error.rs` (add `From<PlanError>`-style mapping helpers if warranted — see Step 5)

**Design contract (single source of truth for the write path):**

```rust
// plan.rs
/// A validated, staged event row ready for tx.insert — proof it passed the
/// optimistic + strict-sequential + encode checks. Owns its encoded key bytes
/// and frame; borrows nothing from the tx.
pub(crate) struct StagedRow {
    pub event_key: Vec<u8>,                 // encode_event_key(id, version)
    pub global_key: [u8; 16],               // encode_global_key(global_seq, version)
    pub frame: bytes::Bytes,                // wire::encode_frame(...).value
}

/// The result of planning one stream's run: rows to insert, the stream's new
/// version counter, and the ending global_seq (== start if the run was empty).
pub(crate) struct PlannedRun {
    pub rows: Vec<StagedRow>,
    pub new_version: u64,
    pub ending_global: u64,
}

/// Neutral planner failure — each caller maps it into its own error domain.
pub(crate) enum PlanError {
    /// Optimistic or sequential mismatch. `expected`/`actual` for append's
    /// Conflict; the atomic caller discards them and uses the run index.
    Conflict { expected: Option<Version>, actual: Option<Version> },
    VersionOverflow,
    GlobalSeqOverflow,
    InvalidInput { version: u64, reason: ErrorId<128> },
}
```

`plan_run` folds envelopes once: optimistic check against `current_version`,
strict-sequential via running `checked_add`, per-event `encode_event_key` +
`wire::encode_frame` + `encode_global_key` with a running `global_seq` from
`current_global`. Pure — no `tx`, no `self`.

- [ ] **Step 1: Write failing unit tests for `plan_run` (TDD, mirrors postgres).**

Create `crates/nexus-fjall/src/plan.rs` with the types above and an inline
`#[cfg(test)]` mod. Tests (each `assert_eq!` on exact values, boundaries included):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use nexus::Version;
    use nexus_store::StreamKey;
    use nexus_store::envelope::pending_envelope;

    fn sk() -> StreamKey { StreamKey::from_bytes("s") }
    fn env(v: u64) -> nexus_store::PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E").payload(b"p".as_slice()).unwrap().build()
    }

    #[test]
    fn fresh_stream_three_events_ok() {
        let evs = [env(1), env(2), env(3)];
        let p = plan_run(0, 0, &sk(), &evs).unwrap();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.new_version, 3);
        assert_eq!(p.ending_global, 3);
    }

    #[test]
    fn existing_stream_continues_global() {
        let evs = [env(6), env(7)];
        let p = plan_run(5, 40, &sk(), &evs).unwrap();
        assert_eq!(p.new_version, 7);
        assert_eq!(p.ending_global, 42);
    }

    #[test]
    fn empty_batch_no_rows_counters_unchanged() {
        let p = plan_run(5, 40, &sk(), &[]).unwrap();
        assert!(p.rows.is_empty());
        assert_eq!(p.new_version, 5);
        assert_eq!(p.ending_global, 40);
    }

    #[test]
    fn gapped_batch_conflict() {
        let evs = [env(1), env(3)];
        let e = plan_run(0, 0, &sk(), &evs).unwrap_err();
        match e {
            PlanError::Conflict { expected, actual } => {
                assert_eq!(expected, Version::new(2));
                assert_eq!(actual, Version::new(3));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn out_of_order_batch_conflict() {
        let evs = [env(2), env(1)];
        let e = plan_run(0, 0, &sk(), &evs).unwrap_err();
        assert!(matches!(e, PlanError::Conflict { .. }));
    }

    #[test]
    fn global_seq_overflow_at_ceiling() {
        let evs = [env(1)];
        let e = plan_run(0, u64::MAX, &sk(), &evs).unwrap_err();
        assert!(matches!(e, PlanError::GlobalSeqOverflow));
    }
}
```

`PlanError` needs `#[derive(Debug)]` for the `panic!` formatting.

- [ ] **Step 2: Run tests to verify they fail (no `plan_run` yet).**

Run: `nix develop -c cargo test -p nexus-fjall plan:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function plan_run` (or unresolved `plan` module until
Step 3 adds `mod plan;`). Add `mod plan;` to `lib.rs` now if needed to reach a
compile error rather than a module error.

- [ ] **Step 3: Implement `plan_run` (pure, no fjall).**

In `plan.rs`, above the tests:

```rust
use nexus::{ErrorId, Version};
use nexus_store::{PendingEnvelope, StreamKey, wire};

use crate::wire_key::{encode_event_key, encode_global_key};

pub(crate) struct StagedRow {
    pub event_key: Vec<u8>,
    pub global_key: [u8; 16],
    pub frame: bytes::Bytes,
}

pub(crate) struct PlannedRun {
    pub rows: Vec<StagedRow>,
    pub new_version: u64,
    pub ending_global: u64,
}

#[derive(Debug)]
pub(crate) enum PlanError {
    Conflict { expected: Option<Version>, actual: Option<Version> },
    VersionOverflow,
    GlobalSeqOverflow,
    InvalidInput { version: u64, reason: ErrorId<128> },
}

/// Pure, IO-free plan for one stream's append run. `current_version` is the
/// stream's current max (0 = new); `current_global` is the store-wide counter.
/// Validates the optimistic + strict-sequential contract, then encodes each
/// event's keys + frame, assigning a running GlobalSeq. No fjall, no tx —
/// unit-testable. The optimistic check itself is the caller's (it differs:
/// append's new-stream `expected must be None`, atomic's projected-head); this
/// plans the run body given an already-agreed `current_version` start.
pub(crate) fn plan_run(
    current_version: u64,
    current_global: u64,
    id: &StreamKey,
    envelopes: &[PendingEnvelope],
) -> Result<PlannedRun, PlanError> {
    let mut expected = current_version;
    let mut global_seq = current_global;
    let mut rows = Vec::with_capacity(envelopes.len());

    for env in envelopes {
        expected = expected.checked_add(1).ok_or(PlanError::VersionOverflow)?;
        if env.version().as_u64() != expected {
            return Err(PlanError::Conflict {
                expected: Version::new(expected),
                actual: Some(env.version()),
            });
        }
        global_seq = global_seq.checked_add(1).ok_or(PlanError::GlobalSeqOverflow)?;

        let version = env.version().as_u64();
        let event_key = encode_event_key(id.as_ref(), version)
            .map_err(|e| PlanError::InvalidInput { version, reason: crate::error::reason_label(&e) })?;
        let frame = wire::encode_frame(
            env.schema_version_value(),
            &env.event_type_value(),
            &env.payload_value(),
            env.metadata_value().as_ref(),
        )
        .map_err(|e| PlanError::InvalidInput { version, reason: crate::error::reason_label(&e) })?;
        let global_key = encode_global_key(global_seq, version);

        rows.push(StagedRow { event_key, global_key, frame: frame.value });
    }

    let new_version = envelopes.last().map_or(current_version, |l| l.version().as_u64());
    Ok(PlannedRun { rows, new_version, ending_global: global_seq })
}
```

- [ ] **Step 4: Run the `plan_run` tests — verify PASS.**

Run: `nix develop -c cargo test -p nexus-fjall plan:: 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 5: Rewrite `RawEventStore::append` as an IO shell over `plan_run`.**

In `store.rs`, replace the body of `append` (lines ~175-281, keeping the
`significant_drop_tightening` allow) with: read current version → `check_optimistic`
(unchanged, keeps the new-stream `expected must be None` semantics) → empty-batch
early return → read current global → `plan_run` → insert staged rows → advance
counters → commit → wake. Map `PlanError` locally:

```rust
let id_bytes = id.as_ref();
let mut tx = self.db.write_tx();

let current_version = self.read_current_version(&tx, id)?;
Self::check_optimistic(current_version, expected_version, id)?;
if envelopes.is_empty() {
    return Ok(());
}
let current_global = self.read_current_global(&tx, id)?;

let planned = plan::plan_run(current_version, current_global, id, envelopes)
    .map_err(|e| append_plan_err(id, e))?;

for row in &planned.rows {
    let slice = Slice::from(row.frame.clone());
    tx.insert(&self.events, &row.event_key, slice.clone());
    tx.insert(&self.events_global, row.global_key, slice);
}
tx.insert(&self.global, GLOBAL_SEQ_KEY, planned.ending_global.to_le_bytes());
tx.insert(&self.streams, id_bytes, encode_stream_version(planned.new_version));

tx.commit().map_err(|e| AppendError::Store(FjallError::Io(e)))?;
self.notifiers.wake(id_bytes);
self.notifiers.wake_all();
Ok(())
```

Add the mapping fn near `append` (free fn in `store.rs`):

```rust
/// Map a neutral `PlanError` into the single-stream `AppendError` domain.
fn append_plan_err(id: &StreamKey, e: plan::PlanError) -> AppendError<FjallError> {
    use plan::PlanError;
    match e {
        PlanError::Conflict { expected, actual } => AppendError::Conflict {
            stream_id: ErrorId::from_display(id),
            expected,
            actual,
        },
        PlanError::VersionOverflow => AppendError::Store(FjallError::VersionOverflow),
        PlanError::GlobalSeqOverflow => AppendError::Store(FjallError::GlobalSeqOverflow),
        PlanError::InvalidInput { version, reason } => {
            AppendError::Store(FjallError::InvalidInput {
                stream_id: ErrorId::from_display(id),
                version,
                reason,
            })
        }
    }
}
```

Note: preserve the pre-#266 append behaviour that the *global counter is only
advanced on a non-empty batch* — the empty-batch early return above guarantees
`planned.rows` is non-empty here, so unconditionally writing `ending_global` matches
the old `global_seq != current_global` outcome (they are equal only when empty,
which returned early). Confirmed equivalent.

Add `use crate::plan;` to the `store.rs` imports.

- [ ] **Step 6: Rewrite the atomic path (`write_atomic_runs`) over `plan_run`.**

In `atomic_append_impl`, keep `validate_atomic_writes` (its optimistic +
projected-head logic is distinct — index-based conflicts across runs), but replace
the per-event encode/stage body of `write_atomic_runs` with a `plan_run` call per
run, threading the running global:

```rust
fn write_atomic_runs(
    &self,
    tx: &mut Tx<'_>,
    writes: &[PlannedAppend],
) -> Result<(), AtomicAppendError<FjallError>> {
    let mut global = self
        .read_global_raw(tx, &writes[0].target)
        .map_err(AtomicAppendError::Store)?;
    for (index, w) in writes.iter().enumerate() {
        // validate_atomic_writes already checked heads; plan the run body from
        // its current head. The stream's current version for the run start is
        // (first event version - 1); recompute from the read head is unnecessary
        // because validate_atomic_writes proved sequentiality from expected+1.
        let current_version = w.expected_version.map_or(0, Version::as_u64);
        let planned = plan::plan_run(current_version, global, &w.target, &w.events)
            .map_err(|e| atomic_plan_err(index, &w.target, e))?;
        let id_bytes = w.target.as_ref();
        for row in &planned.rows {
            let slice = Slice::from(row.frame.clone());
            tx.insert(&self.events, &row.event_key, slice.clone());
            tx.insert(&self.events_global, row.global_key, slice);
        }
        if !planned.rows.is_empty() {
            tx.insert(&self.streams, id_bytes, encode_stream_version(planned.new_version));
        }
        global = planned.ending_global;
    }
    if global != /* starting */ self.read_global_raw(tx, &writes[0].target).map_err(AtomicAppendError::Store)? {
        tx.insert(&self.global, GLOBAL_SEQ_KEY, global.to_le_bytes());
    }
    Ok(())
}
```

REFINEMENT to avoid the double `read_global_raw`: capture the start value once:

```rust
    let start_global = self.read_global_raw(tx, &writes[0].target).map_err(AtomicAppendError::Store)?;
    let mut global = start_global;
    // ... loop ...
    if global != start_global {
        tx.insert(&self.global, GLOBAL_SEQ_KEY, global.to_le_bytes());
    }
```

Add the atomic mapping fn (in `atomic_append_impl`):

```rust
/// Map a neutral `PlanError` into the `AtomicAppendError` domain. A run-body
/// sequential violation surfaced here (after validate_atomic_writes) is an
/// internal invariant break — but map defensively to Conflict at `index`
/// rather than panic (rule 4: no panic on data paths).
fn atomic_plan_err(index: usize, target: &StreamKey, e: plan::PlanError) -> AtomicAppendError<FjallError> {
    use plan::PlanError;
    match e {
        PlanError::Conflict { .. } => AtomicAppendError::Conflict { index, actual: None },
        PlanError::VersionOverflow => AtomicAppendError::Store(FjallError::VersionOverflow),
        PlanError::GlobalSeqOverflow => AtomicAppendError::Store(FjallError::GlobalSeqOverflow),
        PlanError::InvalidInput { version, reason } => AtomicAppendError::Store(FjallError::InvalidInput {
            stream_id: ErrorId::from_display(target),
            version,
            reason,
        }),
    }
}
```

`AtomicAppendError::Conflict` currently carries `{ index, actual: Version }` — verify
its exact fields (`grep -n 'enum AtomicAppendError' -A15 crates/nexus-store/src/import.rs`)
and match them; if `actual` is non-optional, pass `Version::new(current_version)` or
the run's failing version as the existing code did (`conflict()` closure returned
`Version::new(actual)`). Keep the exact prior shape — this arm should be unreachable
in practice since `validate_atomic_writes` runs first.

Import `use super::plan;` (or `crate::plan`) into `atomic_append_impl`.

- [ ] **Step 7: Test the full fjall suite incl. atomic/import + conformance.**

Run: `nix develop -c cargo test -p nexus-fjall --all-features 2>&1 | tail -30`
Expected: PASS — especially `export_import_tests`, `resilience_tests`,
`state_machine_tests`, `property_tests`, and the shared conformance suites. These
are the safety net for the write-path unification.

- [ ] **Step 8: Commit.**

```bash
git add crates/nexus-fjall/src/plan.rs crates/nexus-fjall/src/lib.rs \
        crates/nexus-fjall/src/store.rs
git commit
```

Message:
```
refactor(fjall): one pure plan_run behind append + atomic_append_many

The write path was written twice — RawEventStore::append and the AtomicAppend
impl each re-implemented optimistic + strict-sequential validation and the
per-event encode/dual-partition stage loop, in two error domains kept in
lock-step by hand. Extract a DB-free plan_run (mirrors postgres prepare_inserts):
it validates + encodes + assigns the running GlobalSeq, returning staged rows +
new counters, unit-testable with no fjall. Both public methods are now thin IO
shells that map a neutral PlanError into their own domain. Behaviour-preserving;
guarded by the conformance + export/import + resilience suites.

Relates to #270.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 4: Relocate inline roundtrip tests (finding ⑤)

`store.rs` carries ~80 inline tests; most are *roundtrip* (public `append`/
`read_stream` via `temp_store()`). Move those to `tests/`; keep inline only the
`plan_run` unit tests (Task 3) and the white-box corruption tests that plant rows
into `pub(crate)` partitions directly. This is what actually shrinks `store.rs`.

**Files:**
- Create: `crates/nexus-fjall/tests/append_roundtrip_tests.rs` (relocated public-API tests)
- Modify: `crates/nexus-fjall/src/store.rs` (remove relocated tests; keep white-box mod + `read_test_helpers`)

- [ ] **Step 1: Classify the inline tests.**

Run: `nix develop -c sed -n '684,1754p' crates/nexus-fjall/src/store.rs | grep -nE '#\[tokio::test|#\[test\]|fn |self\.events_global|self\.events\b|self\.global\b|\.inner\(\)'`
For each test: if it touches only `store.append(...)` / `store.read_stream(...)` /
`store.read_all(...)` / `list_streams` / snapshot public API → **relocatable**. If
it reaches `store.events_global`, `store.global`, `store.streams`, `.inner()`, or
plants raw rows (the `frame_value` helper) → **white-box, stays inline**.

- [ ] **Step 2: Create the relocated integration test file.**

Create `crates/nexus-fjall/tests/append_roundtrip_tests.rs` with the standard
integration-test preamble and the relocatable tests moved verbatim. The `sk` and
`temp_store` helpers use only the public API, so redefine them locally at the top:

```rust
//! Roundtrip tests for FjallStore's public append/read API, relocated from the
//! store.rs inline test module (#270) so the production file reads thin. White-box
//! corruption tests that plant partition rows directly remain inline in store.rs.
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::panic, reason = "test code")]

use nexus::Version;
use nexus_store::PendingEnvelope;
use nexus_store::StreamKey;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_fjall::FjallStore;

fn sk(s: &str) -> StreamKey { StreamKey::from_slice(s.as_bytes()) }

fn temp_store() -> (FjallStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

// ... relocated #[tokio::test] fns moved verbatim ...
```

Move each relocatable test body **verbatim** (do not retype logic — copy exactly)
under this preamble, adjusting only the helper source (local `sk`/`temp_store`) and
any `super::`/`crate::` paths to the public `nexus_fjall::` path.

- [ ] **Step 3: Remove the relocated tests from `store.rs`.**

Delete the moved test fns from the inline `mod tests` (store.rs). Keep: the
white-box corruption tests, `frame_value`, and any helper the remaining inline
tests still use. If `read_test_helpers` (`sk`/`temp_store`/`seed`) is now unused by
the trimmed inline mod, delete it too; if still used, keep it.

- [ ] **Step 4: Verify test COUNT is preserved (no silent loss).**

Run: `nix develop -c cargo test -p nexus-fjall --all-features 2>&1 | grep -E 'running [0-9]+ test|test result'`
Expected: the sum of tests across the inline mod + new `append_roundtrip_tests.rs`
equals the prior total (record the pre-move total first). No test silently dropped
(rule 8 — no silent truncation).

- [ ] **Step 5: Confirm the LOC shrink.**

Run: `nix develop -c wc -l crates/nexus-fjall/src/store.rs`
Expected: materially smaller (target ~700-900, down from 1754), with the write-path
production code + white-box tests + `plan_run` tests remaining.

- [ ] **Step 6: Commit.**

```bash
git add crates/nexus-fjall/src/store.rs crates/nexus-fjall/tests/append_roundtrip_tests.rs
git commit
```

Message:
```
test(fjall): relocate roundtrip tests to tests/, keep white-box inline

~80 inline store.rs tests were mostly public-API roundtrips; move them to
tests/append_roundtrip_tests.rs so the production file reads thin like
nexus-postgres. Only the white-box corruption tests (that plant partition
rows directly) and the plan_run unit tests stay inline. Test count preserved.

Relates to #270.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 5: Final verification + PR

- [ ] **Step 1: Full gate green (let the pre-commit hook run it on the last commit; do NOT run `nix flake check` by hand).**

Confirm the last commit's hook passed. If a commit was made with `--no-verify`
anywhere, re-run the commit path so the gate runs.

- [ ] **Step 2: Confirm the example still builds.**

Run: `nix develop -c cargo build -p fjall-end-to-end` (or the example's package name)
Expected: PASS.

- [ ] **Step 3: Update the findings doc with the outcome.**

Append a short "Outcome" section to `docs/plans/2026-07-01-fjall-craftsmanship-findings.md`
recording LOC before/after, allows before/after, and any behaviour note. Commit
(`docs(fjall): record craftsmanship pass outcome`).

- [ ] **Step 4: Push + open PR (joeldsouzax gh account, squash-merge policy).**

```bash
git push -u origin feat/fjall-craftsmanship
gh pr create --title "refactor(fjall): craftsmanship pass — pure plan_run, drop DecodedEvent, visibility, test relocation (#270)" --body "<summary + findings link + test evidence>"
```

---

## Self-Review

- **Spec coverage:** ① pure planner → Task 3. ② `DecodedEvent` → Task 1. ③ `pub mod`
  → Task 2. ④ de-`pub` → Task 2. ⑤ test relocation → Task 4. All five sign-off items
  covered.
- **Type consistency:** `plan_run` / `PlanError` / `StagedRow` / `PlannedRun` used
  consistently across Task 3 steps; `append_plan_err` / `atomic_plan_err` map into
  the existing `AppendError` / `AtomicAppendError` domains. `wire::DecodedFrame` (not
  `DecodedEvent`) consumed in Task 1 `build_envelope`.
- **Placeholders:** none — every code step carries real code. The two spots requiring
  a verify-against-source (`AtomicAppendError::Conflict` fields in Task 3 Step 6;
  exact `PersistedEnvelope::try_new` arg order in Task 1) are flagged with the grep
  to run, not left vague.
- **Risk ordering:** pure/visibility first (1,2), structural refactor (3) under the
  inline+conformance safety net, test relocation last (4) so nothing moves before
  it has protected the refactor.
