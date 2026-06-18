# Aggregate Test Fixture (given / when / then) — Design

**Issue:** devrandom-labs/nexus#126
**Status:** approved design, pre-implementation
**Date:** 2026-06-17

## Goal

Give nexus users a zero-infrastructure way to test their aggregates' domain logic: seed prior history, issue a command, and assert on the decided events, the rejection error, or the resulting state — with no store, codec, or serialization. The fixture drives the **real load path** (rehydration via `replay`), so it catches version/replay bugs as well as decision bugs, not just the pure decide function. The fixture holds an `AggregateRoot<A>` directly and dispatches commands through `root.handle(cmd)` (which calls `Aggregate::handle(state, cmd)`).

```rust
// decided events
AggregateFixture::<BankAccount>::new()
    .given([AccountOpened { balance: 100 }])
    .when(Withdraw { amount: 50 })
    .then_expect_events([Withdrawn { amount: 50 }]);

// rejected command
AggregateFixture::<BankAccount>::with_id(acct_id)
    .given([AccountOpened { balance: 30 }])
    .when(Withdraw { amount: 50 })
    .then_expect_error(BankError::InsufficientFunds);

// resulting state, and/or state straight after replay (no command)
AggregateFixture::<BankAccount>::new()
    .given([AccountOpened { balance: 100 }, Withdrawn { amount: 30 }])
    .then_expect_state(|s| assert_eq!(s.balance, 70));
```

## Location & gating

- Lives in the **`nexus` kernel crate**, not `nexus-store`: it tests domain logic (`Handle`, `AggregateState`, `AggregateRoot`), which are kernel concepts. The existing `nexus-store-testing` crate is for persistence-edge testing; this is a different layer.
- Behind a **new `testing` feature** on the `nexus` crate (kernel currently has only `default` and `derive`). New module `crates/nexus/src/testing.rs`, gated `#[cfg(feature = "testing")]`, re-exported as `nexus::testing::AggregateFixture` (and the typestate structs). Zero cost when the feature is off.
- No new dependencies. Assertions use `assert_eq!` / `assert!` (clippy-clean under the workspace `panic`-deny config, unlike a raw `panic!`), giving an expected-vs-actual diff for free.

## Core decision: full rehydration path (issue open-question resolved)

`given` does **not** fold state by calling `AggregateState::apply` directly. It drives `AggregateRoot::<A>::replay(version, &event)` with versions `1..=n` — exactly what the repository does when loading an aggregate. This means the fixture also exercises strict version validation and `apply` together, the way production does. (Chosen over the "pure decide+evolve" interpretation because the kernel's defensive posture treats replay/version correctness as a first-class invariant, and Agency's keri aggregates will lean on the `sn`→`Version` mapping that this path covers — see agency#137.)

## Typestate flow

Three states; illegal orderings do not compile.

```
AggregateFixture<A>  --given(history)-->  Given<A>  --when(cmd)-->  Acted<A, N>
                                            │                          │
                                            └─ then_expect_state       ├─ then_expect_events
                                                                       ├─ then_expect_error / _matching
                                                                       └─ then_expect_state
```

- **`AggregateFixture<A>`** — entry. Holds the id used to construct the root.
  - `fn new() -> Self where A::Id: Default` — id defaults to `A::Id::default()`. Available only when the id type implements `Default`.
  - `fn with_id(id: A::Id) -> Self` — always available; the escape hatch for id types without `Default`.
  - `fn given(self, history: impl IntoIterator<Item = EventOf<A>>) -> Given<A>` — builds `AggregateRoot::<A>::new(id)`, then `replay`s each event with versions `Version::INITIAL`, `+1`, … using checked increment. A `replay` failure (e.g. a malformed test history) panics with the `KernelError` — that is a test-author error surfaced loudly, which is correct for a fixture.

- **`Given<A>`** — root rehydrated to the post-history state.
  - `fn when<C, const N: usize>(self, cmd: C) -> Acted<A, N> where A: Handle<C, N>` — calls `root.handle(cmd)` (which dispatches to `Aggregate::handle(root.state(), cmd)`) and captures the `Result<Events<EventOf<A>, N>, A::Error>`. Keeps the rehydrated root alongside the result so resulting-state can be computed later. `N` is inferred from the `Handle` impl.
  - `fn then_expect_state(self, f: impl FnOnce(&A::State)) -> Self` — runs `f` against the rehydrated state (`root.state()`). Chainable (returns `Self`) so you can assert state and then issue a command.

- **`Acted<A, const N: usize>`** — post-command. Holds the rehydrated root + the handle result.
  - `fn then_expect_events(self, expected: impl IntoIterator<Item = EventOf<A>>) -> Self` — requires the result is `Ok`; compares the produced `Events<EventOf<A>, N>` against `expected` by exact element-wise `PartialEq`, including count. On `Err`, or on mismatch, panics with a diff. Bound: `EventOf<A>: PartialEq + Debug`.
  - `fn then_expect_error(self, expected: A::Error) -> Self` — requires `Err`; exact match. Bound (on this method only): `A::Error: PartialEq`.
  - `fn then_expect_error_matching(self, f: impl FnOnce(&A::Error) -> bool) -> Self` — requires `Err`; asserts `f(&err)`. No `PartialEq` bound — the escape hatch for errors that wrap non-`PartialEq` sources.
  - `fn then_expect_state(self, f: impl FnOnce(&A::State)) -> Self` — computes the resulting state: clone the rehydrated state (`root.state()`), and **if the command succeeded**, fold the decided events into it via `AggregateState::apply`; then run `f` against that state. After a rejected command there are no decided events and `handle` is non-mutating, so the asserted state equals the rehydrated state (i.e. "the rejected command changed nothing"). Chainable.

All `then_expect_*` methods are **chainable** (return `Self`), enabling `…when(cmd).then_expect_events(...).then_expect_state(...)`.

## Behavior details

- **Versions for `given`:** start at `Version::INITIAL` (= 1), increment with `Version::next()` / checked add; never bare arithmetic (workspace rule). An overflow across a test history is effectively impossible but is handled by surfacing the `KernelError`, not by saturating.
- **Resulting state for `Acted::then_expect_state`:** folds the decided events into a clone of the rehydrated state (`root.state().clone()`) via `AggregateState::apply`. State is `Clone` (guaranteed by `AggregateState: Clone`), so cloning to keep the chain re-assertable is sound and cheap for test-sized state.
- **Event comparison:** compares the produced events as a slice against the collected `expected`. Exact count + exact element equality; a length or element mismatch panics with both sequences printed (`Debug`).
- **Failure output:** every assertion failure prints expected vs actual via `assert_eq!`/`assert!` with a message naming which expectation failed (events / error / state).

## Bounds the user pays

| Capability | Bound required |
|---|---|
| Construct via `new()` | `A::Id: Default` (else use `with_id`) |
| `then_expect_events` | `EventOf<A>: PartialEq + Debug` |
| `then_expect_error` (exact) | `A::Error: PartialEq` (Debug already on `Aggregate::Error`) |
| `then_expect_error_matching` | none |
| `then_expect_state` | none beyond the kernel's existing `AggregateState: Debug + Clone` |

Everything is per-method, so a user who only does `then_expect_error_matching` + `then_expect_state` pays no extra bounds at all.

## Issue open-questions — resolved

1. **`given()` accepts `Vec` or `Events<E, N>`?** Neither specifically — `impl IntoIterator<Item = EventOf<A>>` (a `Vec`, array, or iterator all work). History length is arbitrary and unrelated to the command's `N`, so `Events<E, N>` would be the wrong type here.
2. **Verify version tracking?** Yes — that is the whole point of the full-rehydration path (`given` drives `replay`).
3. **Add `given_commands()` (set up state by running commands)?** Out of scope for v1 (YAGNI). It needs a heterogeneous command list and a handle+apply loop; revisit only if a real need appears.
4. **`nexus` vs separate `nexus-testing` crate?** In the `nexus` kernel under a `testing` feature — no new crate.

## Out of scope (deferred)

- Saga / process-manager fixtures (Axon's `SagaTestFixture`).
- Time control / stub schedulers.
- `given_commands()`.

These are noted so the v1 API is shaped to not preclude them, but none are built now.

## Testing strategy (for the fixture itself)

The fixture is a testing tool, so its own tests must prove it both **passes when it should** and **fails when it should** (a fixture that can't fail is worthless). Applying the mandatory four categories:

1. **Sequence/Protocol:** `given→when→then_expect_events`; `given→then_expect_state` (no command); chained `then_expect_events→then_expect_state`; multi-event history replays in order.
2. **Lifecycle:** `new()` (default id) vs `with_id()` produce equivalent results; rehydration from N events yields the same state as the repository load path for the same events.
3. **Defensive boundary:** a `given` history that violates version sequencing surfaces the `KernelError` (panics) rather than silently mis-seeding; `then_expect_events` on a rejected command panics; `then_expect_error` on a successful command panics.
4. **Negative/can-fail proof:** for each `then_expect_*`, a test that deliberately feeds the wrong expectation and asserts (via `#[should_panic]`) that the fixture panics — proving the assertion is real.

Plus targeted cases: exact-vs-`matching` error forms; the "rejected command changed nothing" state assertion; `EventOf`/`Error` without `PartialEq` compile only against the closure/`_matching` paths (a `trybuild`-style or doc-level note, not necessarily a full UI test).

## Sketch (illustrative — exact signatures may shift in implementation)

```rust
pub struct AggregateFixture<A: Aggregate> { id: A::Id }
pub struct Given<A: Aggregate> { root: AggregateRoot<A> }
pub struct Acted<A: Aggregate, const N: usize> {
    root: AggregateRoot<A>,
    result: Result<Events<EventOf<A>, N>, <A as Aggregate>::Error>,
}

impl<A: Aggregate> AggregateFixture<A> {
    pub fn new() -> Self where A::Id: Default { Self { id: A::Id::default() } }
    pub fn with_id(id: A::Id) -> Self { Self { id } }
    pub fn given(self, history: impl IntoIterator<Item = EventOf<A>>) -> Given<A> { /* AggregateRoot::<A>::new(id) + replay 1..=n */ }
}

impl<A: Aggregate> Given<A> {
    pub fn when<C, const N: usize>(self, cmd: C) -> Acted<A, N> where A: Handle<C, N> { /* root.handle(cmd) */ }
    pub fn then_expect_state(self, f: impl FnOnce(&A::State)) -> Self { /* f(root.state()) */ }
}

impl<A: Aggregate, const N: usize> Acted<A, N> {
    pub fn then_expect_events(self, expected: impl IntoIterator<Item = EventOf<A>>) -> Self
        where EventOf<A>: PartialEq + core::fmt::Debug { /* compare Ok events */ }
    pub fn then_expect_error(self, expected: <A as Aggregate>::Error) -> Self
        where <A as Aggregate>::Error: PartialEq { /* compare Err */ }
    pub fn then_expect_error_matching(self, f: impl FnOnce(&<A as Aggregate>::Error) -> bool) -> Self { /* assert f(err) */ }
    pub fn then_expect_state(self, f: impl FnOnce(&A::State)) -> Self { /* clone root.state(), apply decided events if Ok, f(state) */ }
}
```
