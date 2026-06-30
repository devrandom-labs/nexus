# Prompt — `$all` position seam (#266): test-invariant hardening pass

**Branch:** `feat/all-position-seam` (already has the #266 implementation; **do not** start from `main`).
**gh account:** `joeldsouzax`. **Do NOT commit** until the human signs off on the test pass.
**Do NOT run `nix flake check` by hand** — the pre-commit hook runs it. Verify with targeted
`nix develop -c cargo {build,clippy,nextest} -p nexus-store -p nexus-fjall --all-features`.

## Why this pass exists

The #266 implementation (adapter-defined `$all` position; frame V2 drops `global_seq`;
exclusive `$all` resume; `GlobalSeq` moved into `nexus-fjall`) is **complete and gate-green**
(`clippy --lib` clean, `build --all-targets` clean, 530 + 178 tests pass). But the test changes
were **largely mechanical adaptations** of pre-existing tests plus a handful of new-behaviour
tests. The new invariants are **not yet pinned under the project's 4 mandatory test categories**.
This pass fixes that. Read `CLAUDE.md` §7 (4 categories FIRST) and §8 (test quality) before
writing anything — they override default behaviour.

## Locked context (do not re-litigate — these are the contract under test)

- `$all` resume is **exclusive** (`read_all(Some(p))` = strictly after `p`, `Ord`-based, no
  successor). `read_all(None)` = from the very beginning. Per-stream `read_stream(from)` stays
  **inclusive** on `Version` — the asymmetry is intentional (CLAUDE rule 4).
- `read_all` / `subscribe_all` yield **position-tagged** `(AllPosition, PersistedEnvelope)`.
  The envelope carries **no** global position (the `.global_seq()` accessor is gone). The tag is
  the authoritative position (fjall: the `events_global` key; in-memory: the `BTreeMap` key).
- Position is **monotonic but NOT gapless** — an aborted append may burn a value; readers must
  tolerate gaps and never assume `next == prev + 1`.
- Frame **V2** dropped `global_seq` (`HEADER_FIXED_SIZE` 19→11). **V1 decode is retained** for the
  pre-freeze transition (`decode_frame` branches on the leading version byte). Payload stays
  16-byte aligned.
- Adapters: `nexus-fjall::GlobalSeq` and `nexus_store::testing::InMemoryAllPos` both implement
  `nexus_store::AllPosition`. fjall has `ScanCursor::open_empty` for the `Version::MAX` ceiling.

## What is ALREADY tested (do not duplicate — extend/strengthen only)

- `wire.rs`: V2 round-trip, **V1-decode transition** (`decode_reads_a_v1_frame_dropping_its_global_seq`),
  unknown-version reject, alignment, decode-never-panics (adversarial), layout-concrete examples.
- `catchup.rs`: `read_after(None)`, exclusive `read_after(Some)`, reopen-no-redeliver,
  `Version::MAX` empty scan, `$all` delegation + exclusive.
- `subscription_cursor.rs`: catch-up order, live-tail wake, chunk-boundary no-dup/no-gap,
  `$all` order + live append, scan-error propagation.
- `testing.rs` (InMemoryStore): `read_all` order, exclusive resume, `subscribe_all` catchup+live,
  concurrent `subscribe_all` strictly-increasing, monotonic-across-streams.
- `nexus-fjall`: `read_all` order, exclusive, reopen, `$all` decode → tagged, scan ascending,
  atomic-append shares counter, monotonic-across-streams (`resilience_tests`).
- `envelope.rs`: exhaustive accessor suite (now position-free).

## Gaps to close — organised by the 4 mandatory categories

Apply each to **both** adapters where relevant (`InMemoryStore` and `FjallStore`) — adapter parity
is itself an invariant worth a shared/parameterised test.

### 1. Sequence / protocol (multi-step on one object)
- A `$all` subscription/cursor driven across **multiple resume cycles**: read a prefix, checkpoint
  the tag, `read_all(Some(checkpoint))`, repeat — assert no gap, no dup, no skip across the seam.
- `read_all(None)` then `read_all(Some(last))` chained — strictly-after holds at the boundary.
- Interleave per-stream `read_stream` (inclusive) and `$all` `read_all` (exclusive) on the same
  store — confirm the two contracts coexist and the asymmetry holds (a defensive, explicit test).

### 2. Lifecycle (create / close / corrupt / reopen)
- **fjall**: append V2 frames → close → reopen → `read_all(None)` recovers every event with the
  **same positions** (positions persist across reopen). Extend `read_all_survives_reopen` into a
  multi-event, multi-stream, position-asserting version.
- **fjall**: write → corrupt an `events_global` row / a frame value → reopen → `read_all` surfaces
  a typed `FjallError` (poison, no panic, no silent skip). Pin the exact error variant.
- **Cross-version transition**: a store/byte-stream containing a **V1 frame and a V2 frame** decodes
  correctly under the V2 code path (the V1 one discards `global_seq`, both recover
  schema/event_type/payload). This is the load-bearing transition guarantee and currently only the
  isolated V1 frame is tested.

### 3. Defensive boundary (violate upstream guarantees)
- `read_all(Some(MAX))` on a non-empty store → empty, no panic, no re-read of the ceiling event
  (both adapters). (fjall `ScanCursor::open_empty` path — assert it yields nothing.)
- Position newtype boundaries (`GlobalSeq`, `InMemoryAllPos`) as **property tests** including the
  CLAUDE-mandated `0 / 1 / MAX-1 / MAX`: `new(0) == None`, `INITIAL == 1`, `next()` overflow at
  `MAX` returns `None`, `Ord` total order, `as_u64` round-trip.
- Feed `decode_frame` adversarial V2 bytes (truncated header/event_type/metadata, `schema_version`
  == 0, unknown version byte) — every failure a typed `DecodeError`, never a panic. (Some exists in
  `wire.rs`; make sure the V2 layout specifically is exercised, not only random bytes.)
- **Monotonic-but-not-gapless**: simulate a burned position (e.g. an aborted/failed append between
  two successful ones) and assert a `$all` reader tolerates the gap (no assumption of contiguity).
  This stated contract is currently unpinned.

### 4. Linearizability / isolation (real concurrency — `tokio::spawn` + `Barrier`)
- **fjall** `$all` subscription under concurrent writers across multiple streams: every observed
  position is strictly increasing; never a gap/reorder/dup/phantom; snapshot-consistent prefix.
  (InMemory has `subscribe_all_sees_concurrent_appends_across_streams`; fjall lacks the `$all`
  equivalent — add it. The existing fjall concurrency tests are per-stream only.)
- Concurrent `read_all` (one-shot) racing an `atomic_append_many` — reader sees a valid
  position-ordered prefix, never a torn view, never another stream's un-committed events.

## Also in scope for this pass

- **Adapter-parity harness**: a small generic test fn over `RawEventStore + WakeSource` (or a
  macro) exercising the `$all` contract, instantiated for both `InMemoryStore` and `FjallStore`,
  so the two can never silently diverge. Consider relocating the duplicated
  `append_assigns_monotonic_all_position_across_streams` logic into it.
- **Verify the mechanically-adapted tests still bite**: skim every test #266 edited and confirm the
  assertion still fails if the invariant breaks (CLAUDE §8 "can actually fail"). Watch for tests
  that lost their teeth when `.global_seq()` assertions were removed without a tag-based
  replacement.
- **`clippy --all-targets` cleanup** (NOT gate-enforced, but do it): test/bench style lints —
  `string_lit_as_bytes`, `items_after_statements`, `needless_borrow`, `range_plus_one`, etc. Some
  are pre-existing in files #266 never touched (`security_tests.rs`, `integration_tests.rs`,
  `repository_qa_tests.rs`); prioritise the files #266 edited, then decide with the human whether to
  sweep the rest in this PR or a follow-up.
- **Snapshot sanity**: the 6 regenerated `event_value_*.snap` files are V2 (leading `02`, no 8-byte
  `global_seq`, payload 16-aligned). Spot-check they were not corrupted by the `INSTA_UPDATE=always`
  regen; the key/version snapshots must be **unchanged**.

## Test quality bar (CLAUDE §8 — every test must satisfy ALL)

Calls the real SUT (no re-implemented logic, no hand-rolled probe stores where `InMemoryStore`/
`FjallStore` exist); asserts the **specific** correct value (`assert_eq!`, not "something
happened"); can actually fail; concurrent tests have **real** overlap (`Barrier`); property ranges
include boundaries; no `println!`-as-assertion; each invariant pinned **once** in a canonical
location.

## Execution

- Recommended: `superpowers:subagent-driven-development` (fresh subagent per coherent test group +
  two-stage review), or do it directly if grinding mechanical test additions — direct execution
  often beats subagent overhead for test code.
- After each group: `nix develop -c cargo nextest run -p nexus-store -p nexus-fjall --all-features`.
  `cargo fmt --all` before staging. `cargo hakari generate` only if deps/members change (this pass
  shouldn't).
- A separate follow-up (NOT this pass): the **style / composability** refactor of the #266
  production code, and **Task 8** (revise `docs/plans/2026-06-30-postgres-adapter-plan.md` to
  `PgAllPos { txid, seq }`). Keep those out of the test PR.
