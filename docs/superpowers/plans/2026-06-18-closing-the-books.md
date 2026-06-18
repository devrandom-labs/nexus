# Closing the Books — Guide + Runnable Contrast — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close issue #139 (last Tier 1 card) by shipping a doc-only kernel rustdoc module documenting the "Closing the Books" pattern as the preferred alternative to snapshots, plus a runnable `examples/closing-the-books` crate that contrasts a long-lived aggregate against bounded `CashierShift` streams.

**Architecture:** Two artifacts. (1) `crates/nexus/src/closing_the_books.rs` — a `//!`-only module (no code) shipped on docs.rs. (2) `examples/closing-the-books/` — a pure-kernel example (no store/codec/fjall) that drives both aggregates and prints how many events each must replay to read the current float. A one-line intra-doc pointer in `nexus-store`'s `snapshot.rs` links *down* to the kernel guide (the kernel can't link up to the store).

**Tech Stack:** Rust 2024, `nexus` kernel (`derive` + `testing` features), `thiserror`, `nexus::testing::AggregateFixture`, the Nix flake gate (runs automatically on every commit via the pre-commit hook).

---

## Critical constraints (read before starting)

- **Every commit triggers the full `nix flake check` gate** via the pre-commit hook (clippy god-mode, fmt, nextest, doc, hakari, deny, audit). Therefore **every commit must be fully green** — never commit a red/failing test. The TDD "watch it fail" step is done with a local `cargo test` run and is **not** committed; only the green result is committed. **Do not run `nix flake check` by hand** — the hook runs it.
- **New source files must be `git add`-ed** before the commit, or the gate (which operates on the git tree) won't see them and will fail on a missing module.
- **Example crates are still subject to god-mode clippy.** Existing examples suppress the restriction lints locally with `#![allow(..., reason = "...")]` (the `reason` is mandatory — `allow_attributes_without_reason` is denied). The new example mirrors `examples/inmemory`'s allow-header verbatim.
- **Intra-doc link direction:** `nexus-store` may link to `nexus::closing_the_books` (it depends on `nexus`). `nexus` must **not** link to anything in `nexus-store` (no reverse dependency) — reference the snapshot decorator as a plain code span there.
- If a commit's gate surfaces a minor clippy nit in the example (pedantic/nursery), fix it inline and re-commit; the code below mirrors proven patterns from `examples/inmemory` but the gate is the source of truth.

---

## Task 1: Scaffold the example crate + workspace wiring

**Files:**
- Create: `examples/closing-the-books/Cargo.toml`
- Create: `examples/closing-the-books/src/main.rs`
- Modify: `Cargo.toml` (workspace `members`)
- Possibly modify: `crates/workspace-hack/Cargo.toml` (via `cargo hakari generate`)

- [ ] **Step 1: Create the crate manifest**

`examples/closing-the-books/Cargo.toml`:

```toml
[package]
name = "nexus-example-closing-the-books"
version = "0.0.0"
edition.workspace = true
publish = false

[dependencies]
nexus = { path = "../../crates/nexus", features = ["derive", "testing"] }
thiserror = { workspace = true }
workspace-hack = { version = "0.1", path = "../../crates/workspace-hack" }
```

- [ ] **Step 2: Create a minimal `main.rs` that builds clean**

`examples/closing-the-books/src/main.rs`:

```rust
//! Closing the Books — bounded streams vs. a long-lived aggregate.
//!
//! Builds the same cash-register domain two ways and prints how many events
//! each must replay to read the current float. See the `nexus::closing_the_books`
//! module for the narrative.

fn main() {}
```

- [ ] **Step 3: Register the crate in the workspace**

In `Cargo.toml`, add the new example to `members` (keep the list alphabetical within the `examples/` group):

```toml
members = [
    "crates/nexus",
    "crates/nexus-fjall",
    "crates/nexus-macros",
    "crates/nexus-macros/tests/cross_crate_test",
    "crates/nexus-store",
    "crates/nexus-store-testing",
    "crates/workspace-hack",
    "examples/closing-the-books",
    "examples/inmemory",
    "examples/projection-tokio",
    "examples/store-and-kernel",
    "examples/store-inmemory",
]
```

- [ ] **Step 4: Regenerate the workspace-hack crate**

Run: `nix develop -c cargo hakari generate`
Expected: exits 0; may or may not modify `crates/workspace-hack/Cargo.toml`. If it does, that change is part of this commit.

- [ ] **Step 5: Verify the crate builds**

Run: `nix develop -c cargo build -p nexus-example-closing-the-books`
Expected: compiles with no warnings.

- [ ] **Step 6: Commit** (the pre-commit hook runs the full gate)

```bash
git add examples/closing-the-books/Cargo.toml examples/closing-the-books/src/main.rs Cargo.toml crates/workspace-hack/Cargo.toml
git commit -m "chore(example): scaffold closing-the-books example crate (#139)"
```

---

## Task 2: `CashierShift` aggregate + fixture tests + shift demo

This task adds the bounded-stream aggregate, its `given/when/then` tests, the two shared
replay helpers, and a `main` that runs a two-shift carry-forward demo. The anti-pattern aggregate
arrives in Task 3.

**Files:**
- Modify: `examples/closing-the-books/src/main.rs` (replace entire contents)

- [ ] **Step 1: Write the failing tests first**

Replace `examples/closing-the-books/src/main.rs` with the version below. It contains the full
`CashierShift` aggregate, the shared helpers, a `main` that exercises it, and a `#[cfg(test)]`
module. (The tests reference types defined in the same file; before the impls compile the test
build fails — that is the "red" step.)

```rust
//! Closing the Books — bounded streams vs. a long-lived aggregate.
//!
//! Builds the same cash-register domain two ways and prints how many events
//! each must replay to read the current float. See the `nexus::closing_the_books`
//! module for the narrative.

// Relaxed lints for example code — production crates should NOT do this.
#![allow(clippy::unwrap_used, reason = "example code uses unwrap for brevity")]
#![allow(clippy::expect_used, reason = "example code uses expect for clarity")]
#![allow(
    clippy::print_stdout,
    reason = "example code prints to demonstrate output"
)]

use nexus::*;
use std::fmt;

// =============================================================================
// Shared helpers (a tiny stand-in for what a repository does)
// =============================================================================

/// Append decided events to a stream history and advance the in-memory root,
/// mirroring a repository's persist-then-apply step.
fn record<A: Aggregate, const N: usize>(
    root: &mut AggregateRoot<A>,
    history: &mut Vec<VersionedEvent<EventOf<A>>>,
    decided: &Events<EventOf<A>, N>,
) where
    EventOf<A>: Clone,
{
    let base = root.version().map_or(0, |v| v.as_u64());
    for (i, event) in decided.iter().enumerate() {
        let version = Version::new(base + u64::try_from(i).unwrap() + 1).unwrap();
        history.push(VersionedEvent::new(version, event.clone()));
    }
    let new_version = Version::new(base + u64::try_from(decided.len()).unwrap()).unwrap();
    root.advance_version(new_version);
    root.apply_events(decided);
}

/// Rebuild current state from one stream by replaying every event, returning
/// the number of events that had to be replayed.
fn replay_count<A: Aggregate>(id: A::Id, history: &[VersionedEvent<EventOf<A>>]) -> usize {
    let mut root = AggregateRoot::<A>::new(id);
    for versioned in history {
        root.replay(versioned.version(), versioned.event())
            .expect("valid history");
    }
    history.len()
}

// =============================================================================
// Pattern: CashierShift — a bounded, lifecycle-scoped stream
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CashierShiftId {
    register: String,
    shift_number: u32,
    urn: String,
}

impl CashierShiftId {
    fn new(register: &str, shift_number: u32) -> Self {
        Self {
            register: register.to_owned(),
            shift_number,
            urn: format!("urn:cashier_shift:{register}:{shift_number}"),
        }
    }
}

impl fmt::Display for CashierShiftId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.urn)
    }
}

impl Id for CashierShiftId {
    const BYTE_LEN: usize = 0;
}

impl AsRef<[u8]> for CashierShiftId {
    fn as_ref(&self) -> &[u8] {
        self.urn.as_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, DomainEvent)]
enum ShiftEvent {
    Opened(ShiftOpened),
    TransactionRegistered(TransactionRegistered),
    Closed(ShiftClosed),
}

#[derive(Debug, Clone, PartialEq)]
struct ShiftOpened {
    opening_float: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct TransactionRegistered {
    amount: u64,
}

/// The summary ("closing the books") event: a first-class domain event that
/// carries the minimum state the next shift needs to open.
#[derive(Debug, Clone, PartialEq)]
struct ShiftClosed {
    declared_tender: u64,
    overage: u64,
    shortage: u64,
    final_float: u64,
}

#[derive(Default, Debug, Clone)]
struct ShiftState {
    opening_float: u64,
    registered_total: u64,
    is_open: bool,
    closed: bool,
}

impl AggregateState for ShiftState {
    type Event = ShiftEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &ShiftEvent) -> Self {
        match event {
            ShiftEvent::Opened(e) => {
                self.opening_float = e.opening_float;
                self.registered_total = 0;
                self.is_open = true;
            }
            // Example values stay small; production aggregates use checked_add.
            ShiftEvent::TransactionRegistered(e) => {
                self.registered_total += e.amount;
            }
            ShiftEvent::Closed(_) => {
                self.is_open = false;
                self.closed = true;
            }
        }
        self
    }
}

#[nexus::aggregate(state = ShiftState, error = ShiftError, id = CashierShiftId)]
struct CashierShift;

#[derive(Debug, thiserror::Error, PartialEq)]
enum ShiftError {
    #[error("shift already opened")]
    AlreadyOpen,
    #[error("shift is not open")]
    NotOpen,
}

struct OpenShift {
    opening_float: u64,
}

struct RegisterTransaction {
    amount: u64,
}

struct CloseShift {
    declared_tender: u64,
}

impl Handle<OpenShift> for CashierShift {
    fn handle(state: &ShiftState, cmd: OpenShift) -> Result<Events<ShiftEvent>, ShiftError> {
        if state.is_open || state.closed {
            return Err(ShiftError::AlreadyOpen);
        }
        Ok(events![ShiftEvent::Opened(ShiftOpened {
            opening_float: cmd.opening_float,
        })])
    }
}

impl Handle<RegisterTransaction> for CashierShift {
    fn handle(
        state: &ShiftState,
        cmd: RegisterTransaction,
    ) -> Result<Events<ShiftEvent>, ShiftError> {
        if !state.is_open {
            return Err(ShiftError::NotOpen);
        }
        Ok(events![ShiftEvent::TransactionRegistered(
            TransactionRegistered { amount: cmd.amount }
        )])
    }
}

impl Handle<CloseShift> for CashierShift {
    fn handle(state: &ShiftState, cmd: CloseShift) -> Result<Events<ShiftEvent>, ShiftError> {
        if !state.is_open {
            return Err(ShiftError::NotOpen);
        }
        let expected = state.opening_float + state.registered_total;
        let overage = if cmd.declared_tender > expected {
            cmd.declared_tender - expected
        } else {
            0
        };
        let shortage = if expected > cmd.declared_tender {
            expected - cmd.declared_tender
        } else {
            0
        };
        Ok(events![ShiftEvent::Closed(ShiftClosed {
            declared_tender: cmd.declared_tender,
            overage,
            shortage,
            final_float: cmd.declared_tender,
        })])
    }
}

fn run_cashier_shift_demo() {
    println!("=== Closing the Books: CashierShift (bounded streams) ===");

    // Shift #1: open with float 100, register 10 sales of 1 each, then close.
    let shift1_id = CashierShiftId::new("till-1", 1);
    let mut shift1 = AggregateRoot::<CashierShift>::new(shift1_id.clone());
    let mut shift1_stream: Vec<VersionedEvent<ShiftEvent>> = Vec::new();

    let opened = shift1
        .handle(OpenShift { opening_float: 100 })
        .expect("open shift 1");
    record(&mut shift1, &mut shift1_stream, &opened);
    for _ in 0..10 {
        let txn = shift1
            .handle(RegisterTransaction { amount: 1 })
            .expect("register txn");
        record(&mut shift1, &mut shift1_stream, &txn);
    }
    let closed = shift1
        .handle(CloseShift {
            declared_tender: 110,
        })
        .expect("close shift 1");
    let final_float = closed
        .iter()
        .find_map(|e| match e {
            ShiftEvent::Closed(c) => Some(c.final_float),
            ShiftEvent::Opened(_) | ShiftEvent::TransactionRegistered(_) => None,
        })
        .expect("close produced a ShiftClosed event");
    record(&mut shift1, &mut shift1_stream, &closed);
    println!(
        "shift #1 closed: stream length = {}, final_float = {final_float}",
        shift1_stream.len()
    );

    // Shift #2: a brand-new stream, opened from the carried-forward float.
    let shift2_id = CashierShiftId::new(&shift1_id.register, shift1_id.shift_number + 1);
    let mut shift2 = AggregateRoot::<CashierShift>::new(shift2_id.clone());
    let mut shift2_stream: Vec<VersionedEvent<ShiftEvent>> = Vec::new();

    let opened = shift2
        .handle(OpenShift {
            opening_float: final_float,
        })
        .expect("open shift 2");
    record(&mut shift2, &mut shift2_stream, &opened);
    for _ in 0..10 {
        let txn = shift2
            .handle(RegisterTransaction { amount: 1 })
            .expect("register txn");
        record(&mut shift2, &mut shift2_stream, &txn);
    }

    let replayed = replay_count::<CashierShift>(shift2_id.clone(), &shift2_stream);
    println!("shift #2 {shift2_id} opened from carried float {final_float}");
    println!("current float read by replaying ONLY the current shift: {replayed} events");
}

fn main() {
    run_cashier_shift_demo();
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus::testing::AggregateFixture;

    fn fixture() -> AggregateFixture<CashierShift> {
        AggregateFixture::with_id(CashierShiftId::new("till-1", 1))
    }

    #[test]
    fn close_with_exact_tender_has_no_discrepancy() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 150,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 150,
                overage: 0,
                shortage: 0,
                final_float: 150,
            })]);
    }

    #[test]
    fn close_with_surplus_reports_overage() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 160,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 160,
                overage: 10,
                shortage: 0,
                final_float: 160,
            })]);
    }

    #[test]
    fn close_with_deficit_reports_shortage() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 140,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 140,
                overage: 0,
                shortage: 10,
                final_float: 140,
            })]);
    }

    #[test]
    fn close_before_open_is_rejected() {
        let _ = fixture()
            .given(Vec::<ShiftEvent>::new())
            .when(CloseShift {
                declared_tender: 100,
            })
            .then_expect_error(ShiftError::NotOpen);
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-example-closing-the-books`
Expected: 4 tests pass (`close_with_exact_tender_has_no_discrepancy`, `close_with_surplus_reports_overage`, `close_with_deficit_reports_shortage`, `close_before_open_is_rejected`).

- [ ] **Step 3: Run the example to eyeball the demo**

Run: `nix develop -c cargo run -p nexus-example-closing-the-books`
Expected output (numbers exact): a `=== Closing the Books: CashierShift (bounded streams) ===` header, `shift #1 closed: stream length = 12, final_float = 110`, `shift #2 urn:cashier_shift:till-1:2 opened from carried float 110`, and `current float read by replaying ONLY the current shift: 11 events`.

- [ ] **Step 4: Commit** (pre-commit hook runs the full gate; fix any clippy nit it reports, then re-commit)

```bash
git add examples/closing-the-books/src/main.rs
git commit -m "feat(example): CashierShift bounded-stream aggregate + fixture tests (#139)"
```

---

## Task 3: `CashRegister` anti-pattern + executable contrast

Adds the long-lived aggregate and extends `main` to print the side-by-side replay counts.

**Files:**
- Modify: `examples/closing-the-books/src/main.rs` (add the `CashRegister` block; replace `main`)

- [ ] **Step 1: Add the anti-pattern aggregate**

Insert this block into `examples/closing-the-books/src/main.rs` immediately *before* the
`fn run_cashier_shift_demo()` function:

```rust
// =============================================================================
// Anti-pattern: CashRegister — one never-ending stream (would need a snapshot)
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RegisterId(String);

impl fmt::Display for RegisterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Id for RegisterId {
    const BYTE_LEN: usize = 0;
}

impl AsRef<[u8]> for RegisterId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, DomainEvent)]
enum RegisterEvent {
    Opened(RegisterOpened),
    Sale(SaleRegistered),
}

#[derive(Debug, Clone, PartialEq)]
struct RegisterOpened {
    opening_float: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct SaleRegistered {
    amount: u64,
}

#[derive(Default, Debug, Clone)]
struct RegisterState {
    float: u64,
    is_open: bool,
}

impl AggregateState for RegisterState {
    type Event = RegisterEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &RegisterEvent) -> Self {
        match event {
            RegisterEvent::Opened(e) => {
                self.float = e.opening_float;
                self.is_open = true;
            }
            // Example values stay small; production aggregates use checked_add.
            RegisterEvent::Sale(e) => {
                self.float += e.amount;
            }
        }
        self
    }
}

#[nexus::aggregate(state = RegisterState, error = RegisterError, id = RegisterId)]
struct CashRegister;

#[derive(Debug, thiserror::Error, PartialEq)]
enum RegisterError {
    #[error("register already open")]
    AlreadyOpen,
    #[error("register is not open")]
    NotOpen,
}

struct OpenRegister {
    opening_float: u64,
}

struct RegisterSale {
    amount: u64,
}

impl Handle<OpenRegister> for CashRegister {
    fn handle(
        state: &RegisterState,
        cmd: OpenRegister,
    ) -> Result<Events<RegisterEvent>, RegisterError> {
        if state.is_open {
            return Err(RegisterError::AlreadyOpen);
        }
        Ok(events![RegisterEvent::Opened(RegisterOpened {
            opening_float: cmd.opening_float,
        })])
    }
}

impl Handle<RegisterSale> for CashRegister {
    fn handle(
        state: &RegisterState,
        cmd: RegisterSale,
    ) -> Result<Events<RegisterEvent>, RegisterError> {
        if !state.is_open {
            return Err(RegisterError::NotOpen);
        }
        Ok(events![RegisterEvent::Sale(SaleRegistered {
            amount: cmd.amount,
        })])
    }
}

/// Drive 5,000 sales into one never-ending stream and return how many events
/// must be replayed to read the current float.
fn run_long_lived_demo() -> usize {
    let id = RegisterId("till-1".to_owned());
    let mut register = AggregateRoot::<CashRegister>::new(id.clone());
    let mut stream: Vec<VersionedEvent<RegisterEvent>> = Vec::new();

    let opened = register
        .handle(OpenRegister { opening_float: 100 })
        .expect("open register");
    record(&mut register, &mut stream, &opened);
    for _ in 0..5000 {
        let sale = register
            .handle(RegisterSale { amount: 1 })
            .expect("register sale");
        record(&mut register, &mut stream, &sale);
    }

    // To read the current float we must replay the ENTIRE stream.
    replay_count::<CashRegister>(id, &stream)
}
```

- [ ] **Step 2: Replace `main` to print the contrast**

Replace the existing `fn main()` with:

```rust
fn main() {
    let long_lived = run_long_lived_demo();
    println!("=== Anti-pattern: one long-lived CashRegister stream ===");
    println!(
        "long-lived CashRegister -> {long_lived} events in one stream; \
         replay all {long_lived} to read current float"
    );
    println!();
    run_cashier_shift_demo();
    println!();
    println!("Takeaway: the summary event (ShiftClosed) carries the float forward,");
    println!("so each shift is a short, independent stream — no snapshot required.");
}
```

- [ ] **Step 3: Verify tests still pass and the example runs**

Run: `nix develop -c cargo test -p nexus-example-closing-the-books`
Expected: the same 4 tests pass.

Run: `nix develop -c cargo run -p nexus-example-closing-the-books`
Expected: prints `long-lived CashRegister -> 5001 events in one stream; replay all 5001 to read current float`, then the CashierShift demo block showing `11 events`, then the takeaway lines.

- [ ] **Step 4: Commit** (gate runs; fix any clippy nit and re-commit)

```bash
git add examples/closing-the-books/src/main.rs
git commit -m "feat(example): long-lived CashRegister contrast + replay-count output (#139)"
```

---

## Task 4: Kernel rustdoc guide module

**Files:**
- Create: `crates/nexus/src/closing_the_books.rs`
- Modify: `crates/nexus/src/lib.rs` (add `pub mod closing_the_books;`)

- [ ] **Step 1: Create the doc-only module**

`crates/nexus/src/closing_the_books.rs` (note: `//!` docs only, no code items; all intra-doc
links point *within* the `nexus` crate — never to `nexus-store`):

```rust
//! # Closing the Books — bounded streams instead of snapshots
//!
//! "Closing the Books" is a way to model a long-running process so the
//! aggregate's stream stays short — short enough that you never need a snapshot
//! to load it quickly. It is the **preferred** alternative to snapshots in
//! Nexus whenever the domain has a natural cycle.
//!
//! ## The idea
//!
//! The name comes from accounting. At the end of each period — a day, a month,
//! a cashier's shift — you total everything up, write a closing report, and
//! carry only the closing balance into the next period. You do not re-read a
//! year of ledger entries to know today's opening balance.
//!
//! Translate that to event sourcing:
//!
//! 1. Model the process as a **bounded stream** with an explicit start event
//!    and an explicit end event.
//! 2. When the period ends, append a **summary event** holding exactly the
//!    state the next period needs to start — the closing balance, the totals,
//!    nothing more.
//! 3. The next period is a **brand-new stream** (a new aggregate id) opened
//!    from that carried-forward summary.
//!
//! A cash register is the classic example. Instead of one never-ending
//! `CashRegister` stream, model each shift as its own `CashierShift`:
//! `ShiftOpened` (with the opening float) then `TransactionRegistered` events,
//! then `ShiftClosed` — the summary, carrying the declared tender, any
//! overage or shortage, and the final float. The next shift opens a new stream
//! whose opening float is the previous shift's final float.
//!
//! ## Why this beats a snapshot
//!
//! A summary event *looks* like a snapshot — both let you skip replaying old
//! history — but a summary event is a **first-class domain event**:
//!
//! - It flows through the same [`AggregateState::apply`](crate::AggregateState::apply)
//!   fold and the same upcaster pipeline as every other event, so it versions
//!   like any event. A snapshot is opaque bytes the kernel never interprets.
//! - In the store, a snapshot's schema version is a **cache key, not a
//!   migration engine**: change the state's shape, bump the version, and old
//!   snapshots are silently ignored — every aggregate falls back to a full
//!   replay until a fresh snapshot is written. There is no upcasting path for
//!   snapshot bytes. A summary event has none of this; you evolve it with the
//!   same `#[nexus::transforms]` upcasters you already use for events.
//! - It carries the *minimum* state the next period needs, chosen by the
//!   domain — not a dump of whatever happens to sit in the aggregate struct.
//!
//! So reach for `CashierShift` before you reach for the snapshot decorator
//! (`Snapshotting`, in the `nexus-store` crate).
//!
//! ## What Nexus gives you (and what it does not)
//!
//! Closing the Books is a **modeling discipline**, not an API. Nexus ships no
//! "close stream", "archive", or "delete" operation, and needs none:
//!
//! - Starting the next period is just constructing a fresh
//!   [`AggregateRoot`](crate::AggregateRoot) with a new id; the store creates
//!   the stream on first append.
//! - The summary is just a variant in your event enum, derived with
//!   [`DomainEvent`](crate::DomainEvent).
//!
//! See the runnable `examples/closing-the-books` crate for the full contrast:
//! it builds a long-lived register that would need a snapshot and a
//! shift-bounded register side by side, and prints how many events each must
//! replay to read the current float.
//!
//! ## When NOT to use it
//!
//! Short streams are a tool, not a rule. Keep the stream long (and snapshot if
//! you must) when:
//!
//! - **There is no natural cycle.** Don't invent a boundary the business does
//!   not recognise just to shorten a stream.
//! - **Periods overlap.** One register used by several waiters at once has no
//!   single clean "close" — the lifecycle isn't sequential.
//! - **You can't produce predictable ids** for the next period.
//! - **The stream is low-frequency.** A rarely-changing entity (a company
//!   address) does not grow fast enough for length to matter.
//!
//! ## References
//!
//! - Oskar Dudycz, "Closing the Books in Practice":
//!   <https://event-driven.io/en/closing_the_books_in_practice/>
//! - Oskar Dudycz, "Should you always keep streams short in Event Sourcing?":
//!   <https://event-driven.io/en/should_you_always_keep_streams_short/>
//! - Kurrent (EventStoreDB), "Snapshots in Event Sourcing":
//!   <https://www.kurrent.io/blog/snapshots-in-event-sourcing/>
//! - Kurrent (EventStoreDB), "How to Model Event-Sourced Systems Efficiently":
//!   <https://www.kurrent.io/blog/how-to-model-event-sourced-systems-efficiently/>
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `crates/nexus/src/lib.rs`, add the module declaration after the `testing` module block. The
edit changes:

```rust
#[cfg(feature = "testing")]
pub mod testing;

pub use aggregate::{
```

to:

```rust
#[cfg(feature = "testing")]
pub mod testing;

pub mod closing_the_books;

pub use aggregate::{
```

- [ ] **Step 3: Verify docs build with no broken intra-doc links**

Run: `nix develop -c cargo doc -p nexus --no-deps`
Expected: exits 0 with no warnings (broken intra-doc links would warn and, under the gate, fail).

- [ ] **Step 4: Commit** (gate runs `nexus-doc`)

```bash
git add crates/nexus/src/closing_the_books.rs crates/nexus/src/lib.rs
git commit -m "docs(kernel): add closing_the_books guide module (#139)"
```

---

## Task 5: Cross-link from the snapshot decorator + refresh the snapshot design doc

**Files:**
- Modify: `crates/nexus-store/src/snapshot.rs` (add intra-doc pointer on `Snapshotting`)
- Modify: `docs/plans/2026-04-10-snapshot-design.md` (point its three #139 references at the shipped guide)

- [ ] **Step 1: Add the intra-doc pointer in `snapshot.rs`**

Open `crates/nexus-store/src/snapshot.rs`. Find the `Snapshotting` decorator's doc comment, which
begins:

```rust
/// Snapshot-aware repository decorator.
```

Insert this paragraph immediately after that first line (before the existing blank `///` line):

```rust
/// Snapshot-aware repository decorator.
///
/// **Before reaching for snapshots, consider the
/// [Closing the Books](nexus::closing_the_books) pattern** — modeling the
/// aggregate as bounded, lifecycle-scoped streams often removes the need for
/// snapshots entirely.
```

(The `nexus::closing_the_books` link resolves because `nexus-store` depends on `nexus`.)

- [ ] **Step 2: Point the snapshot design doc's #139 references at the guide**

In `docs/plans/2026-04-10-snapshot-design.md`, make these three replacements:

Replace:
```
| 10,000+ | Essential — but also consider "Closing the Books" pattern (#139) |
```
with:
```
| 10,000+ | Essential — but first consider the "Closing the Books" pattern (see the `nexus::closing_the_books` guide module and `examples/closing-the-books`) |
```

Replace:
```
**Preferred alternative:** Short-lived streams with natural lifecycle boundaries (e.g., `CashierShift` instead of `CashRegister`) eliminate the need for snapshots entirely. See #139.
```
with:
```
**Preferred alternative:** Short-lived streams with natural lifecycle boundaries (e.g., `CashierShift` instead of `CashRegister`) eliminate the need for snapshots entirely. See the `nexus::closing_the_books` guide module and the runnable `examples/closing-the-books` crate.
```

Replace:
```
**Alternative: "Closing the Books" (#139):** Design aggregates with natural lifecycle boundaries (e.g., `CashierShift` per shift vs. `CashRegister` forever). When an aggregate completes its lifecycle, archive its stream and start a new one. This eliminates the need for snapshots entirely and is the preferred approach when domain semantics allow it.
```
with:
```
**Alternative: "Closing the Books":** Design aggregates with natural lifecycle boundaries (e.g., `CashierShift` per shift vs. `CashRegister` forever). When an aggregate completes its lifecycle, archive its stream and start a new one. This eliminates the need for snapshots entirely and is the preferred approach when domain semantics allow it. Documented in the `nexus::closing_the_books` guide module with a runnable contrast in `examples/closing-the-books`.
```

- [ ] **Step 3: Verify the store docs build**

Run: `nix develop -c cargo doc -p nexus-store --no-deps --features snapshot`
Expected: exits 0 with no warnings (the new intra-doc link resolves).

- [ ] **Step 4: Commit** (gate runs)

```bash
git add crates/nexus-store/src/snapshot.rs docs/plans/2026-04-10-snapshot-design.md
git commit -m "docs(store): link snapshot decorator to closing_the_books guide (#139)"
```

---

## Final verification

- [ ] **Run the whole example once more end-to-end**

Run: `nix develop -c cargo run -p nexus-example-closing-the-books`
Expected: long-lived prints `5001`, shift prints `11`, takeaway lines present.

- [ ] **Confirm the branch is clean and all five commits landed**

Run: `git log --oneline -6`
Expected: the five task commits (plus the spec commit) on `docs/closing-the-books-139`.

- [ ] **Open the PR** (only when the user asks — see project merge policy: squash-only, signed commits, `gh` as `joeldsouzax`)

---

## Self-Review

**Spec coverage:**
- Kernel rustdoc module guide → Task 4. ✓
- Runnable example showing both approaches → Tasks 2 (CashierShift) + 3 (CashRegister + contrast). ✓
- Carry-forward (`final_float` → next shift) → Task 2 `run_cashier_shift_demo`. ✓
- Snapshot contrast grounded in real `schema_version`-as-cache-key behavior → Task 4 doc body. ✓
- Caveats / when-not-to-use → Task 4 doc body. ✓
- `AggregateFixture` tests on the close logic → Task 2 test module. ✓
- Cross-link in `snapshot.rs` (correct direction) → Task 5 Step 1. ✓
- Flip the three `#139` references in the snapshot design doc → Task 5 Step 2. ✓
- Workspace member + hakari → Task 1. ✓
- References/citations → Task 4 References section. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result. ✓

**Type/name consistency:** `record` and `replay_count` signatures defined in Task 2 are reused unchanged in Task 3. `CashierShiftId::new(register, shift_number)`, `ShiftEvent`/`ShiftOpened`/`TransactionRegistered`/`ShiftClosed`, `ShiftError::{AlreadyOpen, NotOpen}`, and the `OpenShift`/`RegisterTransaction`/`CloseShift` commands are referenced identically in the demo and the tests. The Task 3 `RegisterEvent`/`RegisterError`/`OpenRegister`/`RegisterSale` names are self-consistent and distinct from the shift names. `pub mod closing_the_books;` (Task 4) matches the file `closing_the_books.rs` and the intra-doc link target used in Task 5. ✓
