# Prompt — `$all` position seam (#266): style & composability refactor pass

**Branch:** `feat/all-position-seam` (already has the #266 implementation **and** the
test-hardening pass; **do not** start from `main`).
**gh account:** `joeldsouzax`. **Do NOT commit** until the human signs off.
**Do NOT run `nix flake check` by hand** — the pre-commit hook runs it. Verify with targeted
`nix develop -c cargo {build,clippy,nextest} -p nexus-store -p nexus-fjall -p nexus-store-testing
--all-features` (clippy with `--all-targets`).

## Why this pass exists

The #266 implementation (adapter-defined `$all` position; frame V2 drops `global_seq`; exclusive
`$all` resume; `GlobalSeq` moved into `nexus-fjall`) is **complete and gate-green**, and the
**test-hardening pass is done** (parity harness, position-newtype property tests, fjall lifecycle /
corruption / burned-gap, `$all` concurrency — all pinned under the 4 mandatory categories, and the
exclusivity path is **mutation-verified** to actually bite). What has **not** had a dedicated pass
is the **prose** of the code itself. This pass reads every working-tree change and makes it **flow
like a poem**: composable, allocate-last, no gratuitous forwarding functions, shallow nesting,
top-to-bottom readable. **Behavior must not change** — the hardened test suite is the safety net.

Read `CLAUDE.md` **§6 (Code Style — Functional-First, Allocate-Last)** in full before touching
anything; it overrides default behavior. Also internalize these standing memories:
`feedback_no_gratuitous_code`, `feedback_justify_every_type`,
`feedback_borrow_always_own_only_when_necessary`, `feedback_no_inline_use`,
`feedback_understand_before_rewriting`, `feedback_no_production_code_for_test_infra`.

## The bar (what "flows like a poem" means here — non-negotiable)

1. **Functional-first.** Combinators (`map`/`and_then`/`map_or_else`/`filter`/`fold`) over
   `if`/`else`/`match` when the transform is a simple data flow. Imperative only where it
   *measurably* wins or enables compile-time safety combinators can't express.
2. **Allocate-last, borrow-first.** Every `Vec::new`/`to_owned`/`to_string`/`Box::new`/`.clone()`
   on a hot path must justify itself. `&str` over `String`, `&[u8]` over `Vec<u8>`, stack-bounded
   (`ArrayVec`/`[T; N]`) over heap. Default `&T`; own only across `.await`, when returning owned
   data, or when storing long-lived. The zero-copy `Bytes`/alignment invariants are sacred — do not
   regress them (`check_alignment_in_zero_copy_designs`).
3. **Lazy over eager.** Iterator/stream chains over intermediate `Vec`s. `.collect()` only when the
   collection is the actual value needed.
4. **Shallow nesting.** `let ... else` over `if let ... else { return }`. Early-return the error/edge
   so the happy path is primary and un-indented. Collapse redundant branches (both arms computing
   the same expression except at a boundary → one expression). No nesting-inside-nesting-inside-nesting.
5. **Imports at the top.** Never scatter `use` mid-file; never deep-qualify
   (`crate::error::AppendError::Conflict { .. }`) in a match arm — import the short name.
   (Fn-scoped `use` inside a `#[cfg(test)]` test fn is the codebase's accepted exception.)
6. **All errors via `thiserror`**, one variant = one failure domain, `#[source]`/`#[from]` preserved,
   no `|_|` discards. **Arithmetic stays checked** (no bare `+`/`-` on the production paths,
   no `saturating_*`, no `unwrap_or(sentinel)`). Atomicity (rule 1) and ordering (rule 5) invariants
   are unchanged by a style edit — confirm you didn't accidentally split a transaction or reorder a
   commit.

## The one trap — gratuitous wrapper vs. load-bearing seam

The user's directive: **"no bullshit functions which do nothing but compose."** Apply it precisely,
not bluntly. A helper is **gratuitous** (inline it, delete it) when it is a thin pass-through /
forwarder that adds a call layer without **DRY-at-scale**, **real logic**, a **load-bearing name**,
or **compile-time safety**. A helper **earns its place** (keep it) when it provides one of those.

- **Delete / inline:** a one-call-site wrapper that just renames a call; a `fn` that forwards every
  argument unchanged to one other `fn`; a "helper" that wraps a single combinator already readable
  inline; ceremony that exists only to look tidy.
- **Keep (these are *real* composition, not bullshit):** the `plan`/`execute` split in
  `wire.rs` (each stage independently testable, named seam), `ScanStrategy` + `ScanCursor` in
  `scan.rs` (factors the per-stream/`$all` difference so one cursor serves both), `Catchup`
  (`StreamCatchup`/`AllCatchup`) so the single live loop monomorphizes branch-free, the typestate
  builders (compile-time ordering safety). **Do not gut deliberate architecture** in the name of
  "fewer functions" — that trades a real invariant for a shallower call graph. If unsure whether a
  seam is load-bearing, it stays; surface it to the human instead of deleting.

Net: fewer *gratuitous* layers, **same or stronger** structural seams. The win is readability and
allocation discipline, never the loss of a compile-time guarantee.

## Scope — the working tree

Review **every unstaged change** on the branch (`git status` / `git diff`). Priority order:

1. **Production `src` (the real target):** `crates/nexus-store/src/*` (esp. `wire.rs`, `store.rs`,
   `testing.rs`, `catchup.rs`, `subscription_cursor.rs`, `subscription.rs`, `envelope.rs`,
   `state.rs`, `import.rs`, `export.rs`, `cbor.rs`, `codec.rs`) and `crates/nexus-fjall/src/*`
   (esp. `store.rs`, `scan.rs`, `global_seq.rs`, `wire_key.rs`).
2. **The new test code from the hardening pass** (`nexus-store-testing/src/lib.rs` `$all` harness;
   the new `#[cfg(test)]` blocks; `tests/*.rs` additions). Same bar — it should read like prose, with
   no gratuitous test helpers — but **never weaken an assertion or its bite** to make it prettier
   (CLAUDE §8: tests must still fail when the invariant breaks; the mutation check must still go red).

## Hard constraints (a style edit must preserve ALL of these)

- **No behavior change.** The #266 contract is **locked** (exclusive `$all`, inclusive
  `read_stream`, position-tagged items, monotonic-not-gapless, frame V2, V1 decode retained). Do not
  re-litigate or alter it. No public-API signature changes unless they are pure
  rename-with-no-semantic-change *and* you flag them for sign-off first.
- **Zero-copy & alignment preserved** (payload 16-byte boundary; `Bytes` Arc-sharing; no added
  copy on the fjall read hot path).
- **Error domains, atomicity, arithmetic safety, `#[non_exhaustive]` on public error enums** — all
  unchanged. A style pass must not silently relax a Mandatory Rule.
- **Tests stay green**, including the mutation-sensitivity (the suite must still bite). If a refactor
  makes a test pass *more easily*, you broke the test or the code — stop.

## Process

1. **Understand before rewriting** (`feedback_understand_before_rewriting`): for any non-trivial
   restructure, read the code + its related types, **state your understanding of the seam and why it
   exists, and WAIT for confirmation** before a large rewrite. Small, obvious local cleanups
   (a `let-else`, an inlined trivial wrapper, a combinator swap) don't need a checkpoint — just do them.
2. **File-by-file, small diffs.** After each file (or coherent group), run
   `nix develop -c cargo nextest run -p <crate> --all-features` for the touched crate. `cargo fmt
   --all` before staging. Do **not** chase whitespace by hand.
3. **Deviation log** for a multi-file sweep (`feedback_deviation_log_during_refactors`): note every
   non-obvious divergence, with reason + impact. Clippy-lint-driven micro-edits are **not**
   deviations (`feedback_clippy_compliance_not_a_deviation`).
4. **Direct execution usually beats subagents** for this kind of read-and-polish work
   (`feedback_subagent_prompt_overhead`); reach for a subagent only for a genuinely independent
   parallel file group.

## Verify (per the guardrails — no hand-run `nix flake check`)

- `nix develop -c cargo build -p nexus-store -p nexus-fjall -p nexus-store-testing --all-features`
- `nix develop -c cargo clippy -p nexus-store -p nexus-fjall -p nexus-store-testing --all-features
  --all-targets` — clean (the gate clippy is `--lib` only, so `--all-targets` is the manual catch
  for test/bench-style lints; `project_flake_clippy_lib_only`).
- `nix develop -c cargo nextest run -p nexus-store -p nexus-fjall --all-features` — **all green**.
- `cargo fmt --all`; `cargo hakari generate` only if deps/members changed (this pass shouldn't touch
  either).

## Out of scope (do NOT do here)

- Any behavior / contract change, new feature, or new test invariant.
- Removing the V1 decode branch (that's the separate pre-1.0-freeze task).
- **Task 8** — revising `docs/plans/2026-06-30-postgres-adapter-plan.md` to `PgAllPos { txid, seq }`.
- Re-opening decisions the #266 design + test pass already settled.
