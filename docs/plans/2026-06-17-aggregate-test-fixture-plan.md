# Aggregate Test Fixture (given/when/then) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a zero-infrastructure `given/when/then` test fixture in the `nexus` kernel that drives the real rehydration path (`replay`) before issuing a command, so users test domain logic *and* replay/version correctness with no store, codec, or serialization.

**Architecture:** The fixture holds an `AggregateRoot<A>` directly — it builds `AggregateRoot::<A>::new(id)`, replays the given history via `root.replay(...)`, and dispatches commands via `root.handle(cmd)` (which calls `Aggregate::handle(state, cmd)`). No entity construction is involved: the aggregate redesign (#197) removed `AggregateEntity`/`from_root` and the entity newtype, so the fixture's type param is `A: Aggregate`. The fixture lives in a new `nexus::testing` module behind a `testing` feature, as a typestate chain `AggregateFixture<A>` → `Given<A>` → `Acted<A, N>`. All assertions are clippy-clean (`assert!`/`assert_eq!` with runtime conditions; no `panic!`/`unwrap`/`expect`).

**Tech Stack:** Rust 2024 edition, `nexus` kernel crate, `#[nexus::aggregate]` proc-macro. Build/check via `nix develop -c …`. Feature branch `feat/aggregate-test-fixture` (already created; design doc already committed). Signed commits; `nix flake check` is the gate.

**Design doc:** `docs/plans/2026-06-17-aggregate-test-fixture-design.md`

---

## File Structure

- **`crates/nexus/Cargo.toml`** — add a `testing` feature.
- **`crates/nexus/src/testing.rs`** — NEW: the fixture (`AggregateFixture`, `Given`, `Acted`), gated on `testing`.
- **`crates/nexus/src/lib.rs`** — declare and re-export the gated `testing` module.
- **`crates/nexus/tests/aggregate_fixture_tests.rs`** — NEW: the fixture's own tests (sample aggregate + all four test categories + negative `#[should_panic]` proofs).
- **`CLAUDE.md`** — document the `testing` fixture.

---

## Task 1: Removed by the aggregate redesign (#197)

> **Task 1 no longer exists.** It originally added a required `AggregateEntity::from_root` method (trait + macro + all hand-written impls) so generic code could wrap an `AggregateRoot<Self>` back into the entity newtype. The aggregate redesign (#197) deleted `AggregateEntity`, `from_root`, and the entity newtype entirely: the aggregate is now a bare marker (`struct BankAccount;`) and `#[nexus::aggregate]` emits only `impl Aggregate`. The fixture holds an `AggregateRoot<A>` directly and never constructs an entity, so no `from_root` is needed. Tasks below keep their original numbering; start implementation at **Task 2**.

---

## Task 2: `testing` feature + fixture entry (`AggregateFixture` → `Given`) + `Given::then_expect_state`

**Files:**
- Modify: `crates/nexus/Cargo.toml`
- Create: `crates/nexus/src/testing.rs`
- Modify: `crates/nexus/src/lib.rs`
- Create: `crates/nexus/tests/aggregate_fixture_tests.rs`

- [ ] **Step 1: Add the `testing` feature**

In `crates/nexus/Cargo.toml`, under `[features]` (currently `default = []` and `derive = …`), add:

```toml
testing = []
```

- [ ] **Step 2: Declare and re-export the module**

In `crates/nexus/src/lib.rs`, with the other module declarations:

```rust
#[cfg(feature = "testing")]
pub mod testing;
```

(Do not add a `pub use testing::*` at the crate root — users reach it via `nexus::testing::AggregateFixture`, keeping the fixture namespaced.)

- [ ] **Step 3: Write the failing test for the entry + `given` + `Given::then_expect_state`**

Create `crates/nexus/tests/aggregate_fixture_tests.rs`. This file defines a sample aggregate via the macro and tests the rehydrate-then-assert-state path (no command yet).

```rust
//! Tests for `nexus::testing::AggregateFixture`. Exercises the real
//! given (replay) -> when (handle) -> then chain on a sample aggregate.
#![cfg(all(feature = "testing", feature = "derive"))]
#![allow(clippy::unwrap_used, reason = "test code")]

use nexus::testing::AggregateFixture;
use nexus::{AggregateState, DomainEvent, Handle, Id, Message, events, Events};

// ── Sample domain ────────────────────────────────────────────────
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct CounterId(u64);
impl core::fmt::Display for CounterId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl AsRef<[u8]> for CounterId {
    fn as_ref(&self) -> &[u8] {
        // stable bytes for storage keys; fine for a test id
        Box::leak(self.0.to_le_bytes().to_vec().into_boxed_slice())
    }
}
impl Id for CounterId {}

#[derive(Clone, Debug, PartialEq)]
enum CounterEvent {
    Incremented(u64),
}
impl Message for CounterEvent {}
impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        "Incremented"
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CounterState {
    total: u64,
}
impl AggregateState for CounterState {
    type Event = CounterEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &CounterEvent) -> Self {
        match event {
            CounterEvent::Incremented(by) => {
                self.total = self.total.checked_add(*by).expect("test totals never overflow");
                self
            }
        }
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("counter error")]
enum CounterError {
    Overflow,
}

#[nexus::aggregate(state = CounterState, error = CounterError, id = CounterId)]
struct Counter;

#[derive(Debug)]
struct Increment(u64);

impl Handle<Increment> for Counter {
    fn handle(state: &CounterState, cmd: Increment) -> Result<Events<CounterEvent>, CounterError> {
        state
            .total
            .checked_add(cmd.0)
            .ok_or(CounterError::Overflow)?;
        Ok(events![CounterEvent::Incremented(cmd.0)])
    }
}

// ── Tests ────────────────────────────────────────────────────────
#[test]
fn given_replays_history_then_asserts_state() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(10), CounterEvent::Incremented(5)])
        .then_expect_state(|s| assert_eq!(s.total, 15));
}

#[test]
fn with_id_constructs_equivalently_to_new() {
    AggregateFixture::<Counter>::with_id(CounterId(42))
        .given([CounterEvent::Incremented(7)])
        .then_expect_state(|s| assert_eq!(s.total, 7));
}
```

- [ ] **Step 4: Run it to verify it fails (no `testing` module yet)**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: FAIL to compile — `nexus::testing` unresolved.

- [ ] **Step 5: Implement the entry + `Given`**

Create `crates/nexus/src/testing.rs`:

```rust
//! Zero-infrastructure aggregate test fixture (`given`/`when`/`then`).
//!
//! Drives the real rehydration path: [`given`](AggregateFixture::given)
//! replays a history through [`AggregateRoot::replay`] (strict version
//! validation), then [`when`](Given::when) calls [`Handle::handle`].
//! No store, codec, or serialization. See
//! `docs/plans/2026-06-17-aggregate-test-fixture-design.md`.

use crate::aggregate::{Aggregate, EventOf, Handle};
use crate::aggregate::AggregateState;
use crate::aggregate::AggregateRoot;
use crate::events::Events;
use crate::version::Version;
use core::fmt::Debug;

/// Entry point. Carries the id used to construct the root for replay.
pub struct AggregateFixture<A: Aggregate> {
    id: A::Id,
}

impl<A: Aggregate> AggregateFixture<A>
where
    A::Id: Default,
{
    /// Start a fixture with a default id. Available when `A::Id: Default`.
    #[must_use]
    pub fn new() -> Self {
        Self { id: A::Id::default() }
    }
}

impl<A: Aggregate> Default for AggregateFixture<A>
where
    A::Id: Default,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Aggregate> AggregateFixture<A> {
    /// Start a fixture with an explicit id (escape hatch for ids without `Default`).
    #[must_use]
    pub fn with_id(id: A::Id) -> Self {
        Self { id }
    }

    /// Replay a history onto a fresh aggregate, producing a [`Given`].
    ///
    /// Drives [`AggregateRoot::replay`] with versions `1..=n`. A malformed
    /// history (out-of-sequence version) fails the replay and panics with
    /// the [`KernelError`](crate::KernelError) — a loud, correct failure for
    /// a test author's bad setup.
    #[must_use]
    pub fn given(self, history: impl IntoIterator<Item = EventOf<A>>) -> Given<A> {
        let mut root = AggregateRoot::<A>::new(self.id);
        let mut version = Version::INITIAL;
        for (index, event) in history.into_iter().enumerate() {
            let outcome = root.replay(version, &event);
            assert!(
                outcome.is_ok(),
                "given: replaying history event #{index} at version {version:?} failed: {outcome:?}"
            );
            if let Some(next) = version.next() {
                version = next;
            }
        }
        Given { root }
    }
}

/// State after replaying the given history; before any command.
pub struct Given<A: Aggregate> {
    root: AggregateRoot<A>,
}

impl<A: Aggregate> Given<A> {
    /// Assert against the rehydrated state (no command issued). Chainable.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&A::State)) -> Self {
        assertion(self.root.state());
        self
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: PASS (`given_replays_history_then_asserts_state`, `with_id_constructs_equivalently_to_new`).

- [ ] **Step 7: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus --features "testing derive" --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/nexus/Cargo.toml crates/nexus/src/lib.rs crates/nexus/src/testing.rs crates/nexus/tests/aggregate_fixture_tests.rs
git commit -S -m "feat(kernel): aggregate test fixture — given + Given::then_expect_state

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `Given::when` + `Acted` + `then_expect_events`

**Files:**
- Modify: `crates/nexus/src/testing.rs`
- Modify: `crates/nexus/tests/aggregate_fixture_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/nexus/tests/aggregate_fixture_tests.rs`:

```rust
#[test]
fn when_decides_then_expect_events() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(10)])
        .when(Increment(5))
        .then_expect_events([CounterEvent::Incremented(5)]);
}

#[test]
fn when_on_empty_history_decides_from_initial() {
    AggregateFixture::<Counter>::new()
        .given([])
        .when(Increment(3))
        .then_expect_events([CounterEvent::Incremented(3)]);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests when_`
Expected: FAIL to compile — no `when` / `then_expect_events`.

- [ ] **Step 3: Implement `when` + `Acted` + `then_expect_events`**

In `crates/nexus/src/testing.rs`, add to `impl<A: Aggregate> Given<A>`:

```rust
    /// Issue a command. Calls [`AggregateRoot::handle`] (which dispatches to
    /// [`Aggregate::handle`]) and captures the result.
    #[must_use]
    pub fn when<C, const N: usize>(self, cmd: C) -> Acted<A, N>
    where
        A: Handle<C, N>,
    {
        let result = self.root.handle(cmd);
        Acted { root: self.root, result }
    }
```

Then add the `Acted` type and its first assertion:

```rust
/// State after issuing a command. Holds the rehydrated root and the
/// handle result so resulting-state can be computed lazily.
pub struct Acted<A: Aggregate, const N: usize> {
    root: AggregateRoot<A>,
    result: Result<Events<EventOf<A>, N>, <A as Aggregate>::Error>,
}

impl<A: Aggregate, const N: usize> Acted<A, N> {
    /// Assert the command succeeded and produced exactly `expected`
    /// (order- and count-sensitive). Chainable.
    #[must_use]
    pub fn then_expect_events(self, expected: impl IntoIterator<Item = EventOf<A>>) -> Self
    where
        EventOf<A>: PartialEq + Debug,
    {
        let expected: Vec<EventOf<A>> = expected.into_iter().collect();
        assert!(
            self.result.is_ok(),
            "then_expect_events: command was rejected with error: {:?}",
            self.result.as_ref().err()
        );
        let Ok(produced) = &self.result else {
            return self; // unreachable: the assert above diverges on Err
        };
        let produced_refs: Vec<&EventOf<A>> = produced.iter().collect();
        let expected_refs: Vec<&EventOf<A>> = expected.iter().collect();
        assert_eq!(
            produced_refs, expected_refs,
            "then_expect_events: decided events did not match expected"
        );
        self
    }
}
```

Note: `Vec` is available — the kernel is `std` (uses `std::error::Error`). `Events::iter()` yields `&EventOf<A>`; comparing `Vec<&EventOf<A>>` needs only `EventOf<A>: PartialEq + Debug` (no `Clone`).

- [ ] **Step 4: Run to verify pass**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: PASS (all four tests so far).

- [ ] **Step 5: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus --features "testing derive" --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/nexus/src/testing.rs crates/nexus/tests/aggregate_fixture_tests.rs
git commit -S -m "feat(kernel): fixture when + Acted::then_expect_events

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `then_expect_error` + `then_expect_error_matching`

**Files:**
- Modify: `crates/nexus/src/testing.rs`
- Modify: `crates/nexus/tests/aggregate_fixture_tests.rs`

- [ ] **Step 1: Add a rejecting command to the sample aggregate**

The `Counter` so far never rejects. Add a command that can fail, in `crates/nexus/tests/aggregate_fixture_tests.rs` after the `Increment` handler:

```rust
#[derive(Debug)]
struct IncrementBounded { by: u64, max: u64 }

impl Handle<IncrementBounded> for Counter {
    fn handle(state: &CounterState, cmd: IncrementBounded) -> Result<Events<CounterEvent>, CounterError> {
        let next = state
            .total
            .checked_add(cmd.by)
            .ok_or(CounterError::Overflow)?;
        if next > cmd.max {
            return Err(CounterError::Overflow);
        }
        Ok(events![CounterEvent::Incremented(cmd.by)])
    }
}
```

- [ ] **Step 2: Write the failing tests**

```rust
#[test]
fn rejected_command_then_expect_error_exact() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(90)])
        .when(IncrementBounded { by: 20, max: 100 })
        .then_expect_error(CounterError::Overflow);
}

#[test]
fn rejected_command_then_expect_error_matching() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(90)])
        .when(IncrementBounded { by: 20, max: 100 })
        .then_expect_error_matching(|e| matches!(e, CounterError::Overflow));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests rejected_`
Expected: FAIL to compile — methods missing.

- [ ] **Step 4: Implement both error assertions**

In `crates/nexus/src/testing.rs`, add to `impl<A: Aggregate, const N: usize> Acted<A, N>`:

```rust
    /// Assert the command was rejected with exactly `expected`. Chainable.
    #[must_use]
    pub fn then_expect_error(self, expected: <A as Aggregate>::Error) -> Self
    where
        <A as Aggregate>::Error: PartialEq,
    {
        assert!(
            self.result.is_err(),
            "then_expect_error: expected the command to be rejected, but it produced events"
        );
        let Err(actual) = &self.result else {
            return self; // unreachable: the assert above diverges on Ok
        };
        assert_eq!(*actual, expected, "then_expect_error: error did not match expected");
        self
    }

    /// Assert the command was rejected with an error satisfying `predicate`.
    /// No `PartialEq` bound — for errors wrapping non-comparable sources. Chainable.
    #[must_use]
    pub fn then_expect_error_matching(
        self,
        predicate: impl FnOnce(&<A as Aggregate>::Error) -> bool,
    ) -> Self {
        assert!(
            self.result.is_err(),
            "then_expect_error_matching: expected the command to be rejected, but it produced events"
        );
        let Err(actual) = &self.result else {
            return self; // unreachable: the assert above diverges on Ok
        };
        assert!(
            predicate(actual),
            "then_expect_error_matching: error did not satisfy predicate: {actual:?}"
        );
        self
    }
```

Note: `assert_eq!(*actual, expected, …)` needs `Error: Debug`, which is guaranteed by `Aggregate::Error: std::error::Error` (Error requires Debug). The added bound is only `PartialEq`. The `{actual:?}` in the matching variant likewise relies on the guaranteed `Debug`.

- [ ] **Step 5: Run to verify pass**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: PASS (all tests).

- [ ] **Step 6: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus --features "testing derive" --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/nexus/src/testing.rs crates/nexus/tests/aggregate_fixture_tests.rs
git commit -S -m "feat(kernel): fixture then_expect_error + then_expect_error_matching

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `Acted::then_expect_state` (resulting state) + chainability

**Files:**
- Modify: `crates/nexus/src/testing.rs`
- Modify: `crates/nexus/tests/aggregate_fixture_tests.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn resulting_state_reflects_decided_events() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(10)])
        .when(Increment(5))
        .then_expect_state(|s| assert_eq!(s.total, 15));
}

#[test]
fn events_and_state_chained_after_one_command() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(1)])
        .when(Increment(2))
        .then_expect_events([CounterEvent::Incremented(2)])
        .then_expect_state(|s| assert_eq!(s.total, 3));
}

#[test]
fn rejected_command_leaves_state_unchanged() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(90)])
        .when(IncrementBounded { by: 20, max: 100 })
        .then_expect_error(CounterError::Overflow)
        .then_expect_state(|s| assert_eq!(s.total, 90));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests resulting_ events_and_state rejected_command_leaves`
Expected: FAIL to compile — `Acted::then_expect_state` missing.

- [ ] **Step 3: Implement `Acted::then_expect_state`**

In `crates/nexus/src/testing.rs`, add to `impl<A: Aggregate, const N: usize> Acted<A, N>`:

```rust
    /// Assert against the resulting state: the rehydrated state with the
    /// decided events applied (if the command succeeded). After a rejected
    /// command no events were decided and `handle` does not mutate, so the
    /// asserted state equals the rehydrated state. Chainable.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&A::State)) -> Self {
        let mut state = self.root.state().clone();
        if let Ok(events) = &self.result {
            for event in events.iter() {
                state = state.apply(event);
            }
        }
        assertion(&state);
        self
    }
```

Note: `A::State: Clone` is guaranteed by `AggregateState: Clone`; `state.apply(event)` is `AggregateState::apply(self, &Self::Event) -> Self` (consumes + returns), matching the kernel's own fold. `AggregateState` is already imported.

- [ ] **Step 4: Run to verify pass**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: PASS (all tests).

- [ ] **Step 5: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus --features "testing derive" --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/nexus/src/testing.rs crates/nexus/tests/aggregate_fixture_tests.rs
git commit -S -m "feat(kernel): fixture Acted::then_expect_state + chainable assertions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Mandatory test categories + negative (`#[should_panic]`) proofs

The fixture is a testing tool, so its own tests must prove it **fails when it should** — a fixture whose assertions can't fail is worthless. This task adds the four mandatory categories' remaining coverage and the can-fail proofs.

**Files:**
- Modify: `crates/nexus/tests/aggregate_fixture_tests.rs`

- [ ] **Step 1: Write the lifecycle + defensive + negative tests**

Append:

```rust
// ── Lifecycle: rehydration parity ────────────────────────────────
#[test]
fn multi_event_history_replays_in_order() {
    AggregateFixture::<Counter>::with_id(CounterId(1))
        .given([
            CounterEvent::Incremented(1),
            CounterEvent::Incremented(2),
            CounterEvent::Incremented(3),
        ])
        .then_expect_state(|s| assert_eq!(s.total, 6));
}

// ── Defensive: wrong expectations panic (can-fail proofs) ─────────
#[test]
#[should_panic(expected = "then_expect_events")]
fn then_expect_events_panics_on_wrong_events() {
    AggregateFixture::<Counter>::new()
        .given([])
        .when(Increment(5))
        .then_expect_events([CounterEvent::Incremented(999)]);
}

#[test]
#[should_panic(expected = "then_expect_events")]
fn then_expect_events_panics_when_command_rejected() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(90)])
        .when(IncrementBounded { by: 20, max: 100 })
        .then_expect_events([CounterEvent::Incremented(20)]);
}

#[test]
#[should_panic(expected = "then_expect_error")]
fn then_expect_error_panics_when_command_succeeded() {
    AggregateFixture::<Counter>::new()
        .given([])
        .when(Increment(1))
        .then_expect_error(CounterError::Overflow);
}

#[test]
#[should_panic(expected = "then_expect_state")]
fn then_expect_state_panics_on_wrong_state() {
    AggregateFixture::<Counter>::new()
        .given([CounterEvent::Incremented(10)])
        .then_expect_state(|s| assert_eq!(s.total, 999));
}
```

Note: the `#[should_panic(expected = …)]` substrings match the assertion messages written in Tasks 3–5 (each message is prefixed with the method name); `then_expect_state_panics_on_wrong_state` panics via the user's own `assert_eq!` inside the closure, whose default panic message contains `assertion `left == right``, but the test asserts on `"then_expect_state"`? — adjust: for that last test, change `expected` to a substring of the closure's `assert_eq!` output. Use `#[should_panic]` with **no** `expected` for the closure-driven one (the panic comes from the user's `assert_eq!`, not the fixture), OR assert on `"left == right"`. Use bare `#[should_panic]` for `then_expect_state_panics_on_wrong_state`.

Apply that correction: make `then_expect_state_panics_on_wrong_state` use bare `#[should_panic]` (no `expected`), because the panic originates in the caller's closure, not the fixture.

- [ ] **Step 2: Run all fixture tests**

Run: `nix develop -c cargo test -p nexus --features "testing derive" --test aggregate_fixture_tests`
Expected: PASS — all positive tests pass, all `#[should_panic]` tests panic as expected.

- [ ] **Step 3: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus --features "testing derive" --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/nexus/tests/aggregate_fixture_tests.rs
git commit -S -m "test(kernel): fixture lifecycle + defensive + can-fail proofs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Docs + full gate

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Document the fixture**

In `CLAUDE.md`, add a short bullet to the kernel-crate section:

```
- **`testing.rs`** (feature `testing`) — `AggregateFixture<A>` given/when/then test fixture. Drives the real replay path then `Handle::handle`; typestate `AggregateFixture → Given → Acted<N>`; assertions: `then_expect_events` (PartialEq), `then_expect_error` / `then_expect_error_matching`, `then_expect_state` (resulting state), all chainable. No store/codec. See `docs/plans/2026-06-17-aggregate-test-fixture-design.md`.
```

Also add `testing` to the kernel feature-flags list if one is enumerated.

- [ ] **Step 2: Full gate**

Run: `nix develop -c nix flake check`
Expected: PASS across the workspace.

Note: confirm the flake's clippy/test derivations exercise the `testing` feature (most run `--all-features`). If `nix flake check` does **not** build the `nexus` crate with `testing` enabled, the fixture would be uncovered in CI — in that case add a step to enable it in the flake's check inputs and re-run. Do not consider this task done until the fixture's tests are demonstrably run by the gate (or explicitly by `cargo test -p nexus --features "testing derive"` as part of the flake).

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -S -m "docs: document the testing fixture

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- Full rehydration path (B) → Task 2 `given` drives `replay`. ✓
- id optional, gated `new()` + `with_id` → Task 2 (`new` requires `A::Id: Default`, `with_id` unconstrained, `Default` impl for clippy). ✓
- Typestate `AggregateFixture → Given → Acted` → Tasks 2–3. ✓
- `then_expect_events` (PartialEq+Debug), `then_expect_error` (PartialEq) + `_matching` (no bound), `then_expect_state` on both `Given` and `Acted`, chainable → Tasks 3–5. ✓
- Generic construction need → none. The aggregate redesign (#197) deleted `AggregateEntity`/`from_root`/the entity newtype; the fixture holds `AggregateRoot<A>` directly (`A: Aggregate`), so no trait extension is required. The original Task 1 (which added `from_root`) was removed. ✓
- `testing` feature, `nexus::testing` namespacing → Task 2. ✓
- Four test categories + can-fail proofs → Tasks 2–6. ✓
- Deferred items (`given_commands`, saga, time) are out of scope → tracked as #194/#195/#196, no tasks here. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step has complete code. The one self-correcting note (Task 6 Step 1 `then_expect_state_panics_on_wrong_state`) is resolved inline (use bare `#[should_panic]`). The Task 7 flake-feature caveat is an explicit verification instruction, not a placeholder.

**3. Type consistency:** The fixture is generic over `A: Aggregate` (not the deleted `AggregateEntity`) throughout. `AggregateFixture<A>` / `Given<A>` / `Acted<A, N>` hold an `AggregateRoot<A>` (`root` field), built by `given` via `AggregateRoot::<A>::new(id)` + `replay`, and `when` calls `self.root.handle(cmd)`. `Acted<A, const N: usize>` fields (`root`, `result`) and methods (`then_expect_events`/`then_expect_error`/`then_expect_error_matching`/`then_expect_state`) are consistent across Tasks 3–5. `given(impl IntoIterator<Item = EventOf<A>>)` and `then_expect_events(impl IntoIterator<Item = EventOf<A>>)` match. The sample `Counter` is a bare marker (`#[nexus::aggregate(...)] struct Counter;`) whose `Handle` impls take `state: &CounterState` (no `&self`). Bounds: `EventOf<A>: PartialEq + Debug` (events), `<A as Aggregate>::Error: PartialEq` (exact error), `A::State: Clone` (free via `AggregateState`). Sample-aggregate command types (`Increment`, `IncrementBounded`) are defined before the tests that use them.
