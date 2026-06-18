# "Closing the Books" — Kernel Guide + Runnable Contrast Example

**Issue:** devrandom-labs/nexus#139 (Tier 1: Table Stakes — last open card)
**Status:** approved design, pre-implementation
**Date:** 2026-06-18

## Goal

Document the "Closing the Books" pattern as the **preferred** modeling alternative to
aggregate snapshots, and back the prose with an **executable** contrast. Two deliverables:

1. A doc-only rustdoc module in the `nexus` kernel crate (`closing_the_books`) — the guide,
   shipped on docs.rs next to `aggregate`, `events`, etc.
2. A runnable example crate `examples/closing-the-books/` that builds the *same* cash-register
   domain two ways — a long-lived aggregate that would need a snapshot, and bounded
   `CashierShift` streams — and prints the difference in events replayed.

This closes #139, the final open card in the Tier 1 milestone. It is a documentation card:
no kernel/store code changes beyond the doc module and one cross-link.

## Background — the pattern (sourced)

"Closing the Books" comes from accounting: at the end of each cycle the numbers are summarised
and only the closing balance is carried forward — you never re-derive the full history. In
event sourcing this becomes: model a process as a **bounded stream** with explicit start/end
events, and on close write a **summary event** carrying the minimum state the next period needs.

Canonical example (Dudycz): a cash register modelled as a series of `CashierShift` streams —
`ShiftOpened` (with an opening *Float*) → `TransactionRegistered`\* → `ShiftClosed` (computes
overage/shortage + `FinalFloat`). The next shift is a **brand-new stream**
(`urn:cashier_shift:{registerId}:{shiftNumber+1}`) whose opening float is the previous shift's
`FinalFloat`.

Key reframe vs snapshots: a summary event **looks like** a snapshot but is a *first-class domain
event* — versioned and upcastable like any other event, carrying the *minimum* business state
forward, not an opaque dump of aggregate internals.

Caveat (the follow-up article walks back "always keep streams short"): the pattern needs a
*real* lifecycle. It is wrong for overlapping periods (one register, multiple waiters),
unpredictable IDs, and low-frequency streams where truncation buys nothing.

Sources are listed under [References](#references).

## Decisions already made

- **Guide home:** kernel rustdoc module (not `docs/`, not README). Chosen because the pattern is
  a *modeling* concern (aggregate + stream design), which is kernel-level; it ships with the API
  docs where a user is standing when they reach for it.
- **Example thoroughness:** a runnable example crate (not doctests, not prose snippets). The
  issue asks for a *contrast*; an executable "events replayed: 5001 vs 12" is a stronger argument
  than a paragraph, and a compiled crate cannot silently drift from the real API.
- **Example shows both** (approach A): the long-lived anti-pattern *and* the closing-the-books
  version, in one crate, with `main` running both.

## Deliverable 1 — kernel rustdoc module

**New file:** `crates/nexus/src/closing_the_books.rs` — a doc-only module: a `//!` guide with
**no code items**. Declared in `crates/nexus/src/lib.rs` as `pub mod closing_the_books;`.

- **Not feature-gated.** Docs must never disappear behind a flag. Zero code, zero deps, always
  compiled. (The `nexus` crate opts into god-mode clippy via `[lints]`, but a doc-only module has
  no code, so there is nothing to lint.)
- **Content outline** (plain language, per house style — everyday words, short sentences):
  1. *What it is* — accounting origin; the event-sourcing translation (bounded stream + summary
     event carrying minimum state forward).
  2. *Why it beats a snapshot here* — a summary event flows through the same
     `AggregateState::apply` fold and the same upcaster pipeline as every other event, so it
     versions like any event. A snapshot is the only kernel-opaque bytes in the system, and the
     store's `schema_version` is a cache key, not a migration engine: bump it and old snapshots
     become a silent cache-miss → full replay (no upcasting path for snapshot bytes).
  3. *Caveats* — needs a real lifecycle; breaks on overlapping periods and unpredictable IDs;
     pointless on low-frequency streams. Don't force boundaries the domain doesn't have.
  4. *See also* — pointer to the `examples/closing-the-books` crate for the runnable proof.
  5. *References* — the four source URLs.

**Cross-link direction (load-bearing constraint):** the `nexus` kernel does **not** depend on
`nexus-store`, so the guide can *name* the `Snapshotting` decorator in prose but cannot make a
clickable intra-doc link up to it. The link goes the other way: add a one-line pointer in
`nexus-store`'s `snapshot.rs` module docs — "before reaching for this, consider
[`nexus::closing_the_books`]" — which **is** a resolvable intra-doc link because `nexus-store`
depends on `nexus`. This puts discoverability at the snapshot API, where the user needs the advice.

## Deliverable 2 — runnable example `examples/closing-the-books/`

A **pure-kernel** example (depends on `nexus` with `derive` + `testing`, plus `thiserror` and
`workspace-hack`), mirroring `examples/inmemory`: no store/codec/fjall, so the modeling lesson
isn't buried under persistence wiring. The contrast is measured by counting events fed through
the real `AggregateRoot::replay` path.

### Convention constraints (verified against `examples/inmemory`)

- **No `[lints] workspace = true`** in the example's `Cargo.toml`. Existing examples deliberately
  omit it; that is why they may use `println!` (workspace lints deny `print_stdout`) and `.unwrap()`
  (deny `unwrap_used`). The contrast example needs `println!`, so it follows the same convention.
  Arithmetic in `apply` mirrors the existing example's bare-`+=` style for readability; the
  arithmetic-safety rule targets library/production code, and the example notes that in a comment.
- **`Id` impl shape** (mirrors `AccountId`): a newtype with
  `#[derive(Debug, Clone, Hash, PartialEq, Eq)]`, a `Display` impl, `impl Id { const BYTE_LEN: usize = 0; }`,
  and `impl AsRef<[u8]>` returning the underlying bytes.

### Anti-pattern aggregate — `CashRegister` (one never-ending stream)

- **Id:** `RegisterId(String)`.
- **State:** `{ float: u64, is_open: bool }`.
- **Events:** `RegisterOpened { opening_float }`, `TransactionRegistered { amount }`.
- No close event — the stream only grows. To answer "current float" you must replay *everything*.
- `main` opens the register and simulates ~5,000 transactions, replays the whole stream via
  `AggregateRoot::replay`, and prints `events replayed: 5001`.

### Pattern aggregate — `CashierShift` (bounded stream)

- **Id:** `CashierShiftId { register: String, shift_number: u32, urn: String }`, where `urn` is
  precomputed once at construction as `urn:cashier_shift:{register}:{shift_number}`. `Display`
  writes `urn`; `AsRef<[u8]>` returns `urn.as_bytes()` (one stable, owned byte view — no
  per-call formatting). A `CashierShiftId::new(register, shift_number)` constructor builds the urn.
- **State:** `{ opening_float: u64, registered_total: u64, is_open: bool, closed: bool }`.
- **Events:** `ShiftOpened { opening_float }`, `TransactionRegistered { amount }`,
  `ShiftClosed { declared_tender, overage, shortage, final_float }`.
- **Commands + `Handle` impls:**
  - `OpenShift { opening_float }` → error if already opened; else `ShiftOpened`.
  - `RegisterTransaction { amount }` → error if not open or already closed; else `TransactionRegistered`.
  - `CloseShift { declared_tender }` → error if not open. Computes
    `expected = opening_float + registered_total`, then `overage = declared > expected ? declared - expected : 0`,
    `shortage = expected > declared ? expected - declared : 0`, `final_float = declared_tender`.
    Emits `ShiftClosed`. (Comparison-then-subtract avoids underflow without `saturating_sub`.)
- **Carry-forward demo:** after closing shift *n*, `main` opens shift *n+1* as a **new
  `CashierShiftId`** (`shift_number + 1`) whose `opening_float` is the prior shift's `final_float`.
- To answer "current float" you replay only the *current* shift's handful of events. `main` prints
  `events replayed: <small k>` for the current shift.

### `main` output

Runs both models and prints a side-by-side line, e.g.:

```
long-lived CashRegister  -> events replayed to read float: 5001
closing-the-books shift  -> events replayed to read float: 12
```

The contrast is *executed*, not asserted in prose.

### Tests (`#[cfg(test)]`)

Use `nexus::testing::AggregateFixture<CashierShift>` to assert the only non-trivial decision
logic (the `CloseShift` handler), doubling as a demo of the fixture:

- `given [ShiftOpened{100}, TransactionRegistered{50}] when CloseShift{declared_tender:150}`
  → `then_expect_events [ShiftClosed{declared_tender:150, overage:0, shortage:0, final_float:150}]`.
- overage: `declared_tender:160` → `overage:10, shortage:0`.
- shortage: `declared_tender:140` → `overage:0, shortage:10`.
- error: `CloseShift` on a never-opened shift → `then_expect_error_matching`.

These satisfy the test-quality rules: each calls the real `Handle` impl via the fixture, asserts
exact expected events, and fails if the close arithmetic or the open-state guard breaks.

## Files touched

| File | Change |
|------|--------|
| `crates/nexus/src/closing_the_books.rs` | **new** — doc-only `//!` guide module |
| `crates/nexus/src/lib.rs` | add `pub mod closing_the_books;` |
| `crates/nexus-store/src/snapshot.rs` | add intra-doc pointer to `nexus::closing_the_books` in the module/decorator docs |
| `examples/closing-the-books/Cargo.toml` | **new** — example crate manifest (no `[lints]` opt-in) |
| `examples/closing-the-books/src/main.rs` | **new** — both aggregates, `main`, `#[cfg(test)]` |
| `Cargo.toml` (workspace) | add `examples/closing-the-books` to `members` |
| `docs/plans/2026-04-10-snapshot-design.md` | flip its three `#139` references to point at the shipped guide |

New source files must be `git add`-ed before running the gate (untracked files are invisible to
`nix flake check`).

## Non-goals

- No new kernel/store API, no "close stream"/"archive" primitive. Closing the Books in nexus is a
  *modeling discipline* — a new stream is just `AggregateRoot::new(new_id)`; the summary is a
  normal `DomainEvent`. The docs teach a technique, not a function.
- No persistence/snapshot wiring in the example — the contrast is measured by replay count, not by
  standing up a real `SnapshotStore`.
- No README change (the guide home is rustdoc, per the decision above).

## Risks / verification

- **Lint inheritance** — resolved: examples omit `[lints] workspace = true`, so `println!`/`.unwrap()`
  are allowed; the new example follows suit. Confirmed against `examples/inmemory/Cargo.toml`.
- **Cross-crate intra-doc link** — the pointer must live in `nexus-store` (→ `nexus`), never in
  `nexus` (→ `nexus-store`), which would not resolve. Captured above.
- **Gate** — the pre-commit hook runs `nix flake check`; do not pre-run it by hand. Ensure the new
  example builds under the workspace and that `cargo doc` resolves the intra-doc link.

## References

- [Closing the Books in Practice — event-driven.io](https://event-driven.io/en/closing_the_books_in_practice/)
- [Should you always keep streams short? — event-driven.io](https://event-driven.io/en/should_you_always_keep_streams_short/)
- [Snapshots in Event Sourcing — Kurrent/EventStoreDB](https://www.kurrent.io/blog/snapshots-in-event-sourcing/)
- [How to Model Event-Sourced Systems Efficiently — Kurrent/EventStoreDB](https://www.kurrent.io/blog/how-to-model-event-sourced-systems-efficiently/)
