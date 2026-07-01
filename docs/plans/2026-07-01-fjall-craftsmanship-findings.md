# nexus-fjall craftsmanship audit — findings (#270)

Date: 2026-07-01. Method: read every production file in `crates/nexus-fjall/src`,
compared against the `nexus-postgres` exemplar and CLAUDE.md's mandatory rules.
Findings are grounded with `file:line`. **Not vibes** — each is a concrete defect
or a concrete "this is fine, leave it."

## 0. Reframe the premise (measure before believing)

The prompt's headline signals are partly a **measurement artifact**. Verified LOC split:

| file | total | production | inline `#[cfg(test)]` |
|---|---:|---:|---:|
| `store.rs` | 1754 | **~683** | 1071 |
| `wire_key.rs` | 856 | **~240** | 616 |
| `scan.rs` | 448 | ~247 | 201 |

- The "1754 LOC / ~69 functions" for `store.rs` counts ~80 inline test fns. Real
  production surface is ~683 LOC.
- The "~20 `#[allow]`" is **~18 test-code allows** (`reason = "test code"` on
  `#[cfg(test)]` mods — standard, not debt) + **2 production allows**
  (`significant_drop_tightening` at `store.rs:165,526`, both with real reasons —
  the `write_tx` guard is deliberately held across the whole transaction) + 1
  crate-wide `result_large_err` (`lib.rs:58`, identical to postgres, justified).
  **There are effectively zero fixable production suppressions.** Reporting this
  honestly matters: the lint-debt smell is not where the prompt guessed.

So the crate is **not** grossly bloated by LOC. The real defects are structural,
and there are exactly four that matter.

## 1. REAL — the write path is written twice (the big one)

`RawEventStore::append` (`store.rs:169-281`) and the `AtomicAppend` impl
(`validate_atomic_writes` `store.rs:423-467` + `write_atomic_runs`
`store.rs:474-522`) are the **same algorithm expressed twice** in two error
domains:

1. read current version(s)
2. optimistic check + strict-sequential validation via running `checked_add`
3. read the global counter
4. per event: `encode_event_key` + `wire::encode_frame` + insert into
   `events` **and** `events_global`, stamping a running `GlobalSeq`
5. advance stream version counter
6. advance global counter (same tx)
7. commit, then wake

`append` is the **single-run special case** of `atomic_append_many`'s N-run
general case. The duplication is the defect: two copies of the optimistic/
sequential/encode/stage logic that must be kept in lock-step by hand.

**Elegant target (the postgres lesson).** Postgres split `append` into a pure,
DB-free `prepare_inserts` (`nexus-postgres/src/store.rs:281`) + a thin IO shell.
Everything in fjall's step 2 and step 4 is **already pure** — `encode_event_key`,
`encode_frame`, `encode_global_key`, the version arithmetic — none of it touches
`tx`. Extract a pure planner:

```
fn plan_run(current_version: u64, current_global: u64, id: &StreamKey,
            envelopes: &[PendingEnvelope]) -> Result<PlannedRun, PlanError>
```

returning the staged rows (`event_key`, `global_key`, `frame` bytes), the new
stream version, and the ending global_seq — unit-testable with no fjall. Then:

- `append` = read counters → `plan_run` (map `PlanError → AppendError`) →
  insert staged rows → commit → wake.
- `atomic_append_many` = fold `plan_run` across runs threading the running
  global + projected heads (map `PlanError → AtomicAppendError`) → insert →
  commit → wake.

Tension to decide: the two conflict shapes genuinely differ
(`AppendError::Conflict{expected,actual}` vs `AtomicAppendError::Conflict{index}`),
so `PlanError` is neutral and each caller maps it. That mapping layer is the cost
of unifying; it's small and buys one source of truth for the write path.

## 2. REAL — `DecodedEvent` is a gratuitous repack (the `DisplayBytes` lesson)

`wire_key.rs:193-223`: `decode_event_value` calls `wire::decode_frame`, then
copies its result field-for-field into a local `DecodedEvent` struct
(`schema_version` + three offset ranges) that is **structurally identical** to
`wire::DecodedFrame`. Its only consumer is `scan.rs`'s `build_envelope`
(`scan.rs:57-81`), which immediately destructures it back into
`PersistedEnvelope::try_new` args.

This is exactly the postgres `DisplayBytes` newtype we killed — a wrapper that
re-expresses a type that already exists. `build_envelope` can consume
`wire::decode_frame`'s output directly; `DecodedEvent` **and**
`decode_event_value` both delete. Net: ~30 prod LOC + one type gone.

## 3. REAL — `pub mod` leaks internals (rule 4, diverges from postgres)

`lib.rs:63-72`: `pub mod builder; pub mod error; pub mod store;` — so
`nexus_fjall::store::*` / `::builder::*` / `::error::*` are public paths *in
addition* to the `pub use` re-exports on lines 74-78. Postgres uses **`mod`
(private) + selective `pub use`** for every module (`nexus-postgres/src/lib.rs`).
CLAUDE.md rule 4: "`pub mod` leaks internals. Use `mod` with controlled
`pub use`." Fix: `mod builder/error/store` + keep the existing `pub use`.
(Public-API change — needs sign-off, but it's the exemplar's shape.)

## 4. REAL (minor) — over-`pub` internals in `wire_key`

`wire_key` is a private mod, so `pub` is really `pub(crate)`, but three items have
**zero** references outside their own module and don't need `pub`:
`event_key_size` (`wire_key.rs:53`), `STREAM_VERSION_SIZE` (`:47`),
`GLOBAL_KEY_SIZE` (`:121`) — internal to the codecs. De-`pub` for honest surface.

## 5. Judgment calls (no change unless you want them)

- **`ScanStrategy` (scan.rs) — earning its keep.** 3 methods, 2 impls
  (`StreamScan`/`GlobalScan`), shared `build_envelope` tail + one `ScanCursor`.
  It monomorphizes into two branch-free cursors; this is the strategy pattern
  doing real work, not over-abstraction. **Leave it.** (Consumes `DecodedEvent`
  today; finding #2 simplifies its decode tail.)
- **`FjallError` (error.rs) — clean.** 8 distinct-domain variants,
  `#[non_exhaustive]` correctly applied (freeze carve-out), `reason_label`
  justified (selects the wider `ErrorId<128>` cap). No change.
- **Inline test relocation — the honest way to shrink `store.rs`.** ~80 inline
  tests, mostly *roundtrip* (public `append`/`read_stream` via `temp_store()`),
  could move to `tests/`; only the handful of white-box corruption tests (that
  plant rows into `pub(crate)` partitions) + the new pure-`plan_run` unit tests
  must stay inline. This is what actually takes `store.rs` from 1754 → ~700.
  **Scope choice, not a mandate** — moving tests is churn with no behaviour value,
  but it's the difference between "looks bloated" and "is thin like postgres."

## 6. Explicitly NOT defects (don't "fix" these)

- The 2 `significant_drop_tightening` allows — the tx guard is held across the
  transaction by design (rule 1). Keep, reasons are real.
- `result_large_err` crate allow — matches postgres, IoT-motivated. Keep.
- `#[non_exhaustive]` on `FjallError` — required by the freeze carve-out. Keep.
- `GlobalSeq` / `builder` / `partition` / `subscription_id` / `snapshot` — all
  clean, single-responsibility, well-documented. No change.

## Proposed scope (for sign-off)

**Core (high value, low risk):** #2 (kill `DecodedEvent`), #4 (de-`pub`), #1
(extract pure `plan_run`, unify the two write paths on it).
**Optional (churn):** #3 (`pub mod → mod`, public-API change), #5 test relocation
(the only thing that moves the LOC needle much).

Nothing here is a behaviour change except where flagged; the conformance suites
are the safety net.

## Outcome (2026-07-01/02)

Delivered on branch `feat/fjall-craftsmanship`, one gate-green commit per step:

**Craftsmanship pass (findings ①–④):**
1. `refactor(fjall): consume wire::DecodedFrame directly, drop DecodedEvent repack`
   — killed the gratuitous repack + ~350 lines of value-codec tests duplicating
   `nexus_store::wire` (rule 8) + 3 dead error variants.
2. `refactor(fjall): mod + pub use over pub mod; de-pub wire_key internals`
   — module visibility now matches nexus-postgres; fixed the cascading
   `redundant_pub_crate`.
3. `refactor(fjall): one pure plan_run behind append + atomic_append_many`
   — the write path is no longer written twice; pure DB-free `plan_run` + a
   flagged rule-3 fix (append version-overflow → `VersionOverflow`, not `Conflict`).
4. `refactor(fjall): drop stale wire_key test allows` — `#[allow]` net reduction.

**Architecture pass (the deeper design work, on maintainer request):**
5. `refactor(fjall): encapsulate the on-disk layout in a Partitions type`
   — `FjallStore` is `{db, partitions, notifiers}`; all partition I/O flows
   through `Partitions`; the `$all` dual-write is the single `stage_event` site.
6. `feat(fjall): configurable $all index (AllIndex) for produce-and-sync IoT`
   — the `$all` full-frame denormalization was implicit; measured it
   (**9.3 MB denormalized vs 6.8 MB pointer, 20k events @120B → ~27%**, up to ~2×
   for large payloads); made it a builder knob `AllIndex::{Denormalized default,
   Disabled}` (IoT producers reclaim the copy; `read_all` errors explicitly).
   Also added CLAUDE.md **Rule 9** (Architectural Decisions — Measure, Question,
   Decide).
7. `refactor(fjall): events_config tunes both event partitions` — killed the
   config asymmetry (`events` tunable but its `events_global` twin wasn't).

Net: production code de-bloated + de-duplicated, one gratuitous type gone,
visibility fixed, the on-disk layout encapsulated, and the single biggest implicit
architectural decision (`$all` denormalization) made **explicit, measured, and
configurable** for the IoT-first target. All gates green.
