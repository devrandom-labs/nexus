# Saga / Process-Manager Primitive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the pure kernel saga primitive (the dual of an aggregate) plus its `SagaFixture`, with no store/IO.

**Architecture:** A saga reuses all aggregate machinery (`Aggregate`, `AggregateState`, `AggregateRoot`, `replay`, versioning). It adds a `Saga: Aggregate` supertrait (`CorrelationKey`, `Command`, `intent_for`) and a `React<E, N>: Saga` trait (the dual of `Handle<C, N>`) whose `react(state, event) -> Result<Option<Events>, Error>` consumes an upstream event and produces the saga's own events. Outgoing intents are derived 1:1 from those own events via `intent_for` (Model A). A new `AggregateRoot::react` dispatch mirrors `AggregateRoot::handle`. `SagaFixture` mirrors `AggregateFixture`.

**Tech Stack:** Rust (edition 2024), `arrayvec` (`Events`), `thiserror`, Nix flake gate (clippy `--lib`, nextest default-features). Design doc: `docs/plans/2026-06-18-saga-primitive-design.md`.

---

## File Structure

- **Create** `crates/nexus/src/saga.rs` — `Saga` trait, `React` trait, `AggregateRoot::react` dispatch (separate inherent `impl` block), in-module unit tests (hand-rolled sample saga, no macro — mirrors `aggregate.rs::purist_dispatch_tests`).
- **Modify** `crates/nexus/src/lib.rs` — `mod saga;` + `pub use saga::{React, Saga};`.
- **Modify** `crates/nexus/src/testing.rs` — add `SagaFixture`, `SagaGiven`, `SagaReacted` (distinct names; `Given`/`Acted` already taken by the aggregate fixture).
- **Create** `crates/nexus/tests/saga_fixture_tests.rs` — integration tests using `#[nexus::aggregate]` macro + `SagaFixture`, mirroring `aggregate_fixture_tests.rs`; covers the 4 mandatory test categories + direct `correlate` test.

### Naming note (deviation from the design doc's illustrative names)

The design doc sketched the fixture states as `Given<S>`/`Reacted<S, N>`. The aggregate fixture already exports `Given`/`Acted` from `testing.rs`, so to avoid a name clash in the same module the saga states are named **`SagaGiven`** and **`SagaReacted`**. Entry point stays `SagaFixture`.

---

## Task 1: `Saga` + `React` traits and `AggregateRoot::react` dispatch

**Files:**
- Create: `crates/nexus/src/saga.rs`
- Modify: `crates/nexus/src/lib.rs`

- [ ] **Step 1: Create `saga.rs` with the traits, dispatch, and a failing unit-test module**

Create `crates/nexus/src/saga.rs` with the full content below. The `#[cfg(test)]` module references the items being defined, so the crate compiles and the tests are the verification.

```rust
//! Saga / process-manager primitive — the dual of an aggregate.
//!
//! Where an aggregate consumes a **command** and produces its own **events**
//! ([`Handle`](crate::Handle)), a saga consumes an upstream **event** and
//! produces its own **events** ([`React`]). The command an aggregate consumes is
//! never folded into its state — it is transient input; only the produced events
//! fold. The upstream event plays the same role for a saga: it is the transient
//! input to [`React::react`], and the saga folds only its *own* events (one
//! stream), exactly like an aggregate. A saga therefore reuses all aggregate
//! machinery ([`Aggregate`], [`AggregateState`](crate::AggregateState),
//! [`AggregateRoot`], [`replay`](AggregateRoot::replay), version tracking) and
//! adds only three things:
//!
//! 1. [`React::correlate`] — which running instance an incoming event belongs to
//!    (pure key *extraction*; instance *resolution* is the runtime's concern).
//! 2. [`React`] instead of [`Handle`](crate::Handle) — one impl per upstream
//!    event type the saga consumes.
//! 3. [`Saga::intent_for`] — derive at most one outgoing intent from each of the
//!    saga's own events (Model A: commands are a 1:1 projection of events, never
//!    a second output, so a dispatch can never drift from a recorded event).
//!
//! See `docs/plans/2026-06-18-saga-primitive-design.md`.

use crate::aggregate::{Aggregate, AggregateRoot, EventOf};
use crate::event::DomainEvent;
use crate::events::Events;
use crate::message::Message;
use core::fmt::Debug;
use core::hash::Hash;

/// Saga-level contract. A saga **is** an aggregate whose events are its own
/// history, plus how to route incoming events and how each own-event maps to an
/// outgoing intent.
///
/// Implement [`React`] (the dual of [`Handle`](crate::Handle)) for each upstream
/// event type the saga consumes. Because `Saga: Aggregate`, obtain the
/// [`Aggregate`] half from `#[nexus::aggregate(..)]` and hand-write `impl Saga`
/// + `impl React`.
pub trait Saga: Aggregate {
    /// Which running instance an incoming event belongs to.
    ///
    /// The runtime (Agency) resolves key → instance; nexus only extracts the
    /// key (see [`React::correlate`]). Bounded so the runtime can use it as a
    /// map key.
    type CorrelationKey: Clone + Eq + Hash + Send + Sync + Debug + 'static;

    /// The saga's emitted intent vocabulary — *thin* requests, enriched into
    /// fat commands by the runtime before reaching a target aggregate. Not tied
    /// to any target aggregate's command type.
    type Command: Message;

    /// Derive the outgoing intent implied by one of the saga's own events.
    ///
    /// At most one intent per event (`None` for internal-only events such as a
    /// terminal `Completed`). Must be a dumb, total lookup with no branching —
    /// all branching already happened in [`React::react`] when it chose *which*
    /// event to record.
    fn intent_for(event: &EventOf<Self>) -> Option<Self::Command>;
}

/// Per-incoming-event-type handler — the **react** function, dual of
/// [`Handle<C, N>`](crate::Handle).
///
/// The const generic `N` controls how many *additional* own-events beyond the
/// first can be produced; total capacity is `N + 1` (default `N = 0` = exactly
/// one own-event). Implement one `React` per upstream event type the saga
/// consumes (the dual of one `Handle` per command type).
pub trait React<E: DomainEvent, const N: usize = 0>: Saga {
    /// Pure correlation-key extraction for this incoming event type.
    ///
    /// `None` means the event is not routed to any instance of this saga.
    fn correlate(event: &E) -> Option<Self::CorrelationKey>;

    /// React to an incoming upstream event, given the current saga state.
    ///
    /// - `Ok(None)` — not interested / no-op (records nothing); e.g. a duplicate
    ///   or a step already passed.
    /// - `Ok(Some(events))` — the saga's own events (at least one), which advance
    ///   state via [`AggregateState::apply`](crate::AggregateState::apply).
    ///
    /// Takes `&E` (borrow) so events read from a stream are not cloned, mirroring
    /// [`AggregateRoot::replay`]. (`handle` takes the command by value because
    /// the caller constructs it fresh; upstream events are read from elsewhere.)
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the incoming event violates a saga invariant.
    fn react(
        state: &Self::State,
        event: &E,
    ) -> Result<Option<Events<EventOf<Self>, N>>, Self::Error>;
}

impl<A: Aggregate> AggregateRoot<A> {
    /// React to an incoming upstream event against the current state.
    ///
    /// Dispatches to the saga's [`React`] impl, passing the current
    /// [`state`](Self::state). Pure — reads state, returns the produced
    /// own-events (or `None`), mutates nothing. The dual of
    /// [`handle`](Self::handle).
    ///
    /// # Errors
    ///
    /// Returns `A::Error` when the incoming event violates a saga invariant.
    pub fn react<E, const N: usize>(
        &self,
        event: &E,
    ) -> Result<Option<Events<EventOf<A>, N>>, A::Error>
    where
        A: React<E, N>,
    {
        A::react(self.state(), event)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code")]
mod saga_dispatch_tests {
    use super::{React, Saga};
    use crate::aggregate::{Aggregate, AggregateRoot, AggregateState};
    use crate::event::DomainEvent;
    use crate::events;
    use crate::events::Events;
    use crate::id::Id;
    use crate::message::Message;
    use crate::version::Version;

    // ── Saga identity ────────────────────────────────────────────────
    #[derive(Debug, Clone, Hash, PartialEq, Eq, Default)]
    struct OrderId([u8; 8]);
    impl OrderId {
        fn new(n: u64) -> Self {
            Self(n.to_le_bytes())
        }
    }
    impl core::fmt::Display for OrderId {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{}", u64::from_le_bytes(self.0))
        }
    }
    impl AsRef<[u8]> for OrderId {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    impl Id for OrderId {
        const BYTE_LEN: usize = 8;
    }

    // ── The saga's OWN events (its history) ──────────────────────────
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum SagaEvent {
        PaymentRequested,
        OrderCompleted,
    }
    impl Message for SagaEvent {}
    impl DomainEvent for SagaEvent {
        fn name(&self) -> &'static str {
            match self {
                Self::PaymentRequested => "PaymentRequested",
                Self::OrderCompleted => "OrderCompleted",
            }
        }
    }

    // ── The saga's outgoing intent vocabulary (thin commands) ────────
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Intent {
        TakePayment,
    }
    impl Message for Intent {}

    // ── Saga state (folds its OWN events) ────────────────────────────
    #[derive(Debug)]
    struct OrderSagaState {
        payment_requested: bool,
        completed: bool,
    }
    impl AggregateState for OrderSagaState {
        type Event = SagaEvent;
        fn initial() -> Self {
            Self {
                payment_requested: false,
                completed: false,
            }
        }
        fn apply(mut self, event: &SagaEvent) -> Self {
            match event {
                SagaEvent::PaymentRequested => self.payment_requested = true,
                SagaEvent::OrderCompleted => self.completed = true,
            }
            self
        }
    }

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("order saga error")]
    enum OrderSagaError {
        EmptyOrder,
    }

    // ── Upstream events the saga CONSUMES (transient input) ──────────
    #[derive(Debug)]
    struct OrderPlaced {
        id: u64,
        total: u64,
    }
    impl Message for OrderPlaced {}
    impl DomainEvent for OrderPlaced {
        fn name(&self) -> &'static str {
            "OrderPlaced"
        }
    }

    #[derive(Debug)]
    struct PaymentSettled {
        id: u64,
    }
    impl Message for PaymentSettled {}
    impl DomainEvent for PaymentSettled {
        fn name(&self) -> &'static str {
            "PaymentSettled"
        }
    }

    // ── The saga marker ──────────────────────────────────────────────
    struct OrderSaga;
    impl Aggregate for OrderSaga {
        type State = OrderSagaState;
        type Error = OrderSagaError;
        type Id = OrderId;
    }
    impl Saga for OrderSaga {
        type CorrelationKey = u64;
        type Command = Intent;
        fn intent_for(event: &SagaEvent) -> Option<Intent> {
            match event {
                SagaEvent::PaymentRequested => Some(Intent::TakePayment),
                SagaEvent::OrderCompleted => None, // internal-only
            }
        }
    }
    impl React<OrderPlaced> for OrderSaga {
        fn correlate(event: &OrderPlaced) -> Option<u64> {
            Some(event.id)
        }
        fn react(
            state: &OrderSagaState,
            event: &OrderPlaced,
        ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
            if event.total == 0 {
                return Err(OrderSagaError::EmptyOrder);
            }
            if state.payment_requested {
                return Ok(None); // already handled — ignore duplicate
            }
            Ok(Some(events![SagaEvent::PaymentRequested]))
        }
    }
    impl React<PaymentSettled> for OrderSaga {
        fn correlate(event: &PaymentSettled) -> Option<u64> {
            Some(event.id)
        }
        fn react(
            _state: &OrderSagaState,
            _event: &PaymentSettled,
        ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
            Ok(Some(events![SagaEvent::OrderCompleted]))
        }
    }

    #[test]
    fn react_produces_own_events_via_dispatch() {
        let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        let produced = root
            .react(&OrderPlaced { id: 1, total: 100 })
            .expect("ok")
            .expect("some");
        assert_eq!(
            produced.into_iter().collect::<Vec<_>>(),
            vec![SagaEvent::PaymentRequested]
        );
    }

    #[test]
    fn react_ignores_when_no_op() {
        // Replay a history where payment was already requested, then a second
        // OrderPlaced is a duplicate → Ok(None).
        let mut root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        root.replay(Version::INITIAL, &SagaEvent::PaymentRequested)
            .expect("replay");
        let outcome = root
            .react(&OrderPlaced { id: 1, total: 100 })
            .expect("ok");
        assert!(outcome.is_none());
    }

    #[test]
    fn react_surfaces_saga_error() {
        let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        assert_eq!(
            root.react::<OrderPlaced, 0>(&OrderPlaced { id: 1, total: 0 }),
            Err(OrderSagaError::EmptyOrder)
        );
    }

    #[test]
    fn correlate_extracts_key_per_event_type() {
        assert_eq!(
            <OrderSaga as React<OrderPlaced>>::correlate(&OrderPlaced { id: 42, total: 9 }),
            Some(42)
        );
        assert_eq!(
            <OrderSaga as React<PaymentSettled>>::correlate(&PaymentSettled { id: 7 }),
            Some(7)
        );
    }

    #[test]
    fn intent_for_projects_events_one_to_one() {
        assert_eq!(
            OrderSaga::intent_for(&SagaEvent::PaymentRequested),
            Some(Intent::TakePayment)
        );
        assert_eq!(OrderSaga::intent_for(&SagaEvent::OrderCompleted), None);
    }
}
```

- [ ] **Step 2: Wire the module and re-exports in `lib.rs`**

In `crates/nexus/src/lib.rs`, add `saga` to the module list and re-export the traits. Add `mod saga;` after `mod message;`:

```rust
mod aggregate;
mod error;
mod event;
mod events;
mod id;
mod message;
mod saga;
mod version;
```

And add to the `pub use` block (after the `message` re-export):

```rust
pub use saga::{React, Saga};
```

- [ ] **Step 3: Stage the new file (the gate fails on untracked source)**

Run: `git add crates/nexus/src/saga.rs crates/nexus/src/lib.rs`
(Untracked source files make `nix flake check` fail on a missing module — stage before building.)

- [ ] **Step 4: Run the saga unit tests**

Run: `nix develop -c cargo test -p nexus --lib saga_dispatch_tests`
Expected: PASS — 5 tests (`react_produces_own_events_via_dispatch`, `react_ignores_when_no_op`, `react_surfaces_saga_error`, `correlate_extracts_key_per_event_type`, `intent_for_projects_events_one_to_one`).

- [ ] **Step 5: Run clippy on the library**

Run: `nix develop -c cargo clippy -p nexus --lib -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Format and commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus/src/saga.rs crates/nexus/src/lib.rs
git commit -m "feat(kernel): saga primitive — Saga + React traits, AggregateRoot::react (#127)"
```
(The pre-commit hook runs `nix flake check` automatically — do not run it by hand first.)

---

## Task 2: `SagaFixture` test fixture

**Files:**
- Modify: `crates/nexus/src/testing.rs`
- Test: `crates/nexus/tests/saga_fixture_tests.rs` (created in Task 3)

- [ ] **Step 1: Add the fixture imports to `testing.rs`**

At the top of `crates/nexus/src/testing.rs`, the existing imports are:

```rust
use crate::aggregate::{Aggregate, AggregateRoot, EventOf, Handle};
use crate::events::Events;
use crate::version::Version;
use core::fmt::Debug;
```

Change the first line to also import the saga traits and add a `DomainEvent` import:

```rust
use crate::aggregate::{Aggregate, AggregateRoot, EventOf, Handle};
use crate::event::DomainEvent;
use crate::events::Events;
use crate::saga::{React, Saga};
use crate::version::Version;
use core::fmt::Debug;
```

- [ ] **Step 2: Append the `SagaFixture` types to `testing.rs`**

Append the following to the end of `crates/nexus/src/testing.rs`:

```rust
/// Saga test fixture entry point — the dual of [`AggregateFixture`].
///
/// Drives the real replay + react path: [`given`](SagaFixture::given) replays
/// the saga's own past events through [`AggregateRoot::replay`], then
/// [`when`](SagaGiven::when) calls [`AggregateRoot::react`] with an incoming
/// upstream event and projects outgoing intents via [`Saga::intent_for`]. No
/// store, codec, or serialization.
///
/// `correlate` is intentionally not part of the flow: a test already knows which
/// instance it drives (the `given` history *is* that instance). Test it directly:
/// `assert_eq!(<MySaga as React<E>>::correlate(&event), Some(key))`.
pub struct SagaFixture<S: Saga> {
    id: S::Id,
}

impl<S: Saga> SagaFixture<S>
where
    S::Id: Default,
{
    /// Start a fixture with a default id. Available when `S::Id: Default`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: S::Id::default(),
        }
    }
}

impl<S: Saga> Default for SagaFixture<S>
where
    S::Id: Default,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Saga> SagaFixture<S> {
    /// Start a fixture with an explicit id (escape hatch for ids without `Default`).
    #[must_use]
    pub const fn with_id(id: S::Id) -> Self {
        Self { id }
    }

    /// Replay the saga's own history onto a fresh root, producing a [`SagaGiven`].
    ///
    /// Drives [`AggregateRoot::replay`] with versions `1..=n`.
    ///
    /// # Panics
    ///
    /// Panics if replaying any history event fails (e.g. an out-of-sequence
    /// version) — a malformed history is a test-author setup bug.
    #[must_use]
    pub fn given(self, history: impl IntoIterator<Item = EventOf<S>>) -> SagaGiven<S> {
        let mut root = AggregateRoot::<S>::new(self.id);
        let mut version = Version::INITIAL;
        for (index, event) in history.into_iter().enumerate() {
            let outcome = root.replay(version, &event);
            assert!(
                outcome.is_ok(),
                "given: replaying saga history event #{index} at version {version:?} failed: {outcome:?}"
            );
            if let Some(next) = version.next() {
                version = next;
            }
        }
        SagaGiven { root }
    }
}

/// State after replaying the saga's history; before any incoming event.
pub struct SagaGiven<S: Saga> {
    root: AggregateRoot<S>,
}

impl<S: Saga> SagaGiven<S> {
    /// Feed an incoming upstream event. Calls [`AggregateRoot::react`] and — when
    /// it produces own-events — folds them into the root via the no-clone
    /// [`AggregateRoot::apply_events`] (so state assertions need no `Clone`) and
    /// projects each produced event through [`Saga::intent_for`] (in order) to
    /// collect the outgoing intents. On `Ok(None)` or `Err` nothing is folded and
    /// no intents are collected.
    #[must_use]
    pub fn when<E, const N: usize>(self, event: E) -> SagaReacted<S, N>
    where
        S: React<E, N>,
        E: DomainEvent,
    {
        let mut root = self.root;
        let result = root.react::<E, N>(&event);
        let commands = match &result {
            Ok(Some(events)) => {
                let cmds: Vec<S::Command> = events.iter().filter_map(|e| S::intent_for(e)).collect();
                root.apply_events(events);
                cmds
            }
            _ => Vec::new(),
        };
        SagaReacted {
            root,
            result,
            commands,
        }
    }

    /// Assert against the rehydrated state (no event fed). Returns `Self`.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&S::State)) -> Self {
        assertion(self.root.state());
        self
    }
}

/// State after reacting to an incoming event. Holds the (already-folded) root,
/// the raw `react` result, and the projected outgoing intents.
pub struct SagaReacted<S: Saga, const N: usize> {
    root: AggregateRoot<S>,
    result: Result<Option<Events<EventOf<S>, N>>, <S as Aggregate>::Error>,
    commands: Vec<S::Command>,
}

impl<S: Saga, const N: usize> SagaReacted<S, N> {
    /// Assert `react` produced exactly `expected` own-events (order- and
    /// count-sensitive). Returns `Self`.
    ///
    /// # Panics
    ///
    /// Panics if `react` returned `Err`, returned `Ok(None)` (ignored), or
    /// produced events that do not match `expected`.
    #[must_use]
    pub fn then_expect_events(self, expected: impl IntoIterator<Item = EventOf<S>>) -> Self
    where
        EventOf<S>: PartialEq + Debug,
    {
        let expected_events: Vec<EventOf<S>> = expected.into_iter().collect();
        assert!(
            self.result.is_ok(),
            "then_expect_events: react was rejected with error: {:?}",
            self.result.as_ref().err()
        );
        let Ok(maybe) = &self.result else {
            return self; // unreachable: the assert above diverges on Err
        };
        assert!(
            maybe.is_some(),
            "then_expect_events: react ignored the event (Ok(None)) but events were expected"
        );
        let Some(produced) = maybe else {
            return self; // unreachable: the assert above diverges on None
        };
        let produced_refs: Vec<&EventOf<S>> = produced.iter().collect();
        let expected_refs: Vec<&EventOf<S>> = expected_events.iter().collect();
        assert_eq!(
            produced_refs, expected_refs,
            "then_expect_events: produced own-events did not match expected"
        );
        self
    }

    /// Assert the intents projected from the produced events equal `expected`
    /// (order- and count-sensitive). Returns `Self`.
    ///
    /// # Panics
    ///
    /// Panics if the projected intents do not match `expected`.
    #[must_use]
    pub fn then_expect_commands(self, expected: impl IntoIterator<Item = S::Command>) -> Self
    where
        S::Command: PartialEq + Debug,
    {
        let expected_cmds: Vec<S::Command> = expected.into_iter().collect();
        assert_eq!(
            self.commands, expected_cmds,
            "then_expect_commands: projected intents did not match expected"
        );
        self
    }

    /// Assert `react` ignored the event (`Ok(None)`). Returns `Self`.
    ///
    /// # Panics
    ///
    /// Panics if `react` produced events or returned an error.
    #[must_use]
    pub fn then_expect_ignored(self) -> Self {
        assert!(
            matches!(self.result, Ok(None)),
            "then_expect_ignored: expected react to ignore the event (Ok(None)), got: {:?}",
            self.result
        );
        self
    }

    /// Assert `react` was rejected with exactly `expected`. Returns `Self`.
    ///
    /// # Panics
    ///
    /// Panics if `react` succeeded, or the error does not equal `expected`.
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "by-value `expected` mirrors assert_eq! call style; PartialEq only borrows"
    )]
    pub fn then_expect_error(self, expected: <S as Aggregate>::Error) -> Self
    where
        <S as Aggregate>::Error: PartialEq,
    {
        assert!(
            self.result.is_err(),
            "then_expect_error: expected react to be rejected, but it produced: {:?}",
            self.result.as_ref().ok()
        );
        let Err(actual) = &self.result else {
            return self; // unreachable: the assert above diverges on Ok
        };
        assert_eq!(
            *actual, expected,
            "then_expect_error: error did not match expected"
        );
        self
    }

    /// Assert `react` was rejected with an error satisfying `predicate`. No
    /// `PartialEq` bound. Returns `Self`.
    ///
    /// # Panics
    ///
    /// Panics if `react` succeeded, or the error does not satisfy `predicate`.
    #[must_use]
    pub fn then_expect_error_matching(
        self,
        predicate: impl FnOnce(&<S as Aggregate>::Error) -> bool,
    ) -> Self {
        assert!(
            self.result.is_err(),
            "then_expect_error_matching: expected react to be rejected, but it produced: {:?}",
            self.result.as_ref().ok()
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

    /// Assert against the resulting state. [`when`](SagaGiven::when) already
    /// folded any produced events (on `Ok(Some)`), so this is a pure borrow —
    /// no `Clone` bound. After `Ok(None)`/`Err` nothing was folded, so the state
    /// equals the rehydrated state. Returns `Self`.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&S::State)) -> Self {
        assertion(self.root.state());
        self
    }
}
```

- [ ] **Step 3: Run clippy on the library with the `testing` feature**

Run: `nix develop -c cargo clippy -p nexus --lib --features testing -- -D warnings`
Expected: no warnings. (Verifies the fixture compiles under strict clippy before the integration tests exist.)

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus/src/testing.rs
git commit -m "feat(kernel): SagaFixture given/when/then test fixture (#127)"
```

---

## Task 3: Integration tests — the 4 mandatory categories + correlate

**Files:**
- Create: `crates/nexus/tests/saga_fixture_tests.rs`

- [ ] **Step 1: Write the integration test file**

Create `crates/nexus/tests/saga_fixture_tests.rs` with the full content below. It uses the `#[nexus::aggregate]` macro for the `Aggregate` half (mirrors `aggregate_fixture_tests.rs`), then hand-writes `impl Saga` + `impl React`.

```rust
//! Tests for `nexus::testing::SagaFixture`. Exercises the real
//! given (replay) -> when (react) -> then chain on a sample saga, plus the
//! 4 mandatory cross-cutting categories (sequence, lifecycle, defensive
//! boundary; linearizability is N/A at the pure-primitive layer).
#![cfg(all(feature = "testing", feature = "derive"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    unused_must_use,
    reason = "test code; terminal then_expect_* assertions intentionally drop the returned fixture"
)]

use nexus::testing::SagaFixture;
use nexus::{AggregateState, DomainEvent, Events, Id, Message, React, Saga, events};

// ── Saga identity ────────────────────────────────────────────────
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct OrderId([u8; 8]);
impl OrderId {
    const fn new(n: u64) -> Self {
        Self(n.to_le_bytes())
    }
}
impl core::fmt::Display for OrderId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", u64::from_le_bytes(self.0))
    }
}
impl AsRef<[u8]> for OrderId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Id for OrderId {
    const BYTE_LEN: usize = 8;
}

// ── The saga's OWN events (its history) ──────────────────────────
#[derive(Clone, Debug, PartialEq, Eq)]
enum SagaEvent {
    PaymentRequested,
    OrderCompleted,
}
impl Message for SagaEvent {}
impl DomainEvent for SagaEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::PaymentRequested => "PaymentRequested",
            Self::OrderCompleted => "OrderCompleted",
        }
    }
}

// ── The saga's outgoing intents (thin commands) ──────────────────
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    TakePayment,
}
impl Message for Intent {}

// ── Saga state (folds its OWN events) ────────────────────────────
#[derive(Clone, Debug, Default, PartialEq)]
struct OrderSagaState {
    payment_requested: bool,
    completed: bool,
}
impl AggregateState for OrderSagaState {
    type Event = SagaEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &SagaEvent) -> Self {
        match event {
            SagaEvent::PaymentRequested => self.payment_requested = true,
            SagaEvent::OrderCompleted => self.completed = true,
        }
        self
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("order saga error")]
enum OrderSagaError {
    EmptyOrder,
}

// ── Upstream events the saga CONSUMES (transient input) ──────────
#[derive(Debug)]
struct OrderPlaced {
    id: u64,
    total: u64,
}
impl Message for OrderPlaced {}
impl DomainEvent for OrderPlaced {
    fn name(&self) -> &'static str {
        "OrderPlaced"
    }
}

#[derive(Debug)]
struct PaymentSettled {
    id: u64,
}
impl Message for PaymentSettled {}
impl DomainEvent for PaymentSettled {
    fn name(&self) -> &'static str {
        "PaymentSettled"
    }
}

// ── The saga marker (Aggregate half via macro; Saga + React by hand) ─
#[nexus::aggregate(state = OrderSagaState, error = OrderSagaError, id = OrderId)]
struct OrderSaga;

impl Saga for OrderSaga {
    type CorrelationKey = u64;
    type Command = Intent;
    fn intent_for(event: &SagaEvent) -> Option<Intent> {
        match event {
            SagaEvent::PaymentRequested => Some(Intent::TakePayment),
            SagaEvent::OrderCompleted => None,
        }
    }
}

impl React<OrderPlaced> for OrderSaga {
    fn correlate(event: &OrderPlaced) -> Option<u64> {
        Some(event.id)
    }
    fn react(
        state: &OrderSagaState,
        event: &OrderPlaced,
    ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
        if event.total == 0 {
            return Err(OrderSagaError::EmptyOrder);
        }
        if state.payment_requested {
            return Ok(None);
        }
        Ok(Some(events![SagaEvent::PaymentRequested]))
    }
}

impl React<PaymentSettled> for OrderSaga {
    fn correlate(event: &PaymentSettled) -> Option<u64> {
        Some(event.id)
    }
    fn react(
        _state: &OrderSagaState,
        _event: &PaymentSettled,
    ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
        Ok(Some(events![SagaEvent::OrderCompleted]))
    }
}

// ── Basic given/when/then ────────────────────────────────────────
#[test]
fn react_on_empty_history_produces_event_and_intent() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(OrderPlaced { id: 1, total: 100 })
        .then_expect_events([SagaEvent::PaymentRequested])
        .then_expect_commands([Intent::TakePayment])
        .then_expect_state(|s| assert!(s.payment_requested));
}

#[test]
fn with_id_constructs_equivalently_to_new() {
    SagaFixture::<OrderSaga>::with_id(OrderId::new(42))
        .given([])
        .when(OrderPlaced { id: 42, total: 5 })
        .then_expect_events([SagaEvent::PaymentRequested]);
}

#[test]
fn internal_only_event_projects_no_intent() {
    // PaymentSettled -> OrderCompleted, which has no outgoing intent.
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested])
        .when(PaymentSettled { id: 1 })
        .then_expect_events([SagaEvent::OrderCompleted])
        .then_expect_commands([])
        .then_expect_state(|s| assert!(s.completed));
}

// ── Category 1: Sequence/Protocol ────────────────────────────────
#[test]
fn sequence_react_after_prior_own_history() {
    // The saga already requested payment (its own history); a duplicate
    // OrderPlaced must be ignored, leaving state unchanged.
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested])
        .when(OrderPlaced { id: 1, total: 100 })
        .then_expect_ignored()
        .then_expect_state(|s| assert!(s.payment_requested && !s.completed));
}

// ── Category 2: Lifecycle (replay to a known state, then react) ──
#[test]
fn lifecycle_replay_full_history_then_react() {
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested, SagaEvent::OrderCompleted])
        .then_expect_state(|s| assert!(s.payment_requested && s.completed));
}

#[test]
#[should_panic(expected = "replaying saga history event")]
fn lifecycle_malformed_history_panics_in_given() {
    // `given` always feeds versions 1..=n in order, so the realistic replay
    // failure is the rehydration limit. A saga with MAX_REHYDRATION_EVENTS = 1
    // fed two events makes `replay` return Err, which `given` asserts on and
    // panics. See the `small_limit` module below.
    small_limit::trigger_panic();
}

mod small_limit {
    use super::{Intent, OrderId};
    use nexus::testing::SagaFixture;
    use nexus::{Aggregate, AggregateState, DomainEvent, Message, Saga};
    use std::num::NonZeroUsize;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Ev {
        Tick,
    }
    impl Message for Ev {}
    impl DomainEvent for Ev {
        fn name(&self) -> &'static str {
            "Tick"
        }
    }
    #[derive(Clone, Debug, Default)]
    struct St;
    impl AggregateState for St {
        type Event = Ev;
        fn initial() -> Self {
            Self
        }
        fn apply(self, _: &Ev) -> Self {
            self
        }
    }
    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("e")]
    struct Err1;
    struct Tiny;
    impl Aggregate for Tiny {
        type State = St;
        type Error = Err1;
        type Id = OrderId;
        const MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(1).unwrap();
    }
    impl Saga for Tiny {
        type CorrelationKey = u64;
        type Command = Intent;
        fn intent_for(_: &Ev) -> Option<Intent> {
            None
        }
    }

    pub fn trigger_panic() {
        // Replaying 2 events exceeds MAX_REHYDRATION_EVENTS = 1 → replay Err →
        // `given` asserts and panics with "replaying saga history event".
        let _ = SagaFixture::<Tiny>::new().given([Ev::Tick, Ev::Tick]);
    }
}

// ── Category 3: Defensive boundary ───────────────────────────────
#[test]
fn boundary_react_rejects_invalid_event_exact() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(OrderPlaced { id: 1, total: 0 })
        .then_expect_error(OrderSagaError::EmptyOrder)
        .then_expect_state(|s| assert!(!s.payment_requested));
}

#[test]
fn boundary_react_rejects_invalid_event_matching() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(OrderPlaced { id: 1, total: 0 })
        .then_expect_error_matching(|e| matches!(e, OrderSagaError::EmptyOrder));
}

// ── correlate (pure, tested directly — not via the fixture flow) ──
#[test]
fn correlate_extracts_key_per_upstream_event_type() {
    assert_eq!(
        <OrderSaga as React<OrderPlaced>>::correlate(&OrderPlaced { id: 7, total: 1 }),
        Some(7)
    );
    assert_eq!(
        <OrderSaga as React<PaymentSettled>>::correlate(&PaymentSettled { id: 9 }),
        Some(9)
    );
}
```

- [ ] **Step 2: Run the integration tests**

Run: `nix develop -c cargo test -p nexus --test saga_fixture_tests --features testing,derive`
Expected: PASS — all tests, including the `#[should_panic]` lifecycle test.

- [ ] **Step 3: Clippy on the test target**

Run: `nix develop -c cargo clippy -p nexus --test saga_fixture_tests --features testing,derive -- -D warnings`
Expected: no warnings. (The flake clippy is `--lib` only, so test-target clippy must be run by hand.)

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus/tests/saga_fixture_tests.rs
git commit -m "test(kernel): saga fixture integration tests across mandatory categories (#127)"
```

---

## Task 4: Documentation — CLAUDE.md kernel section

**Files:**
- Modify: `crates/nexus/CLAUDE.md` is not the file — update the root `CLAUDE.md` kernel section.

- [ ] **Step 1: Add a `saga.rs` bullet to the Kernel Crate module list**

In `/Users/joel/Code/devrandom/nexus/CLAUDE.md`, in the "Kernel Crate (`nexus`) — Flat Module Layout" section, add a bullet after the `aggregate.rs` bullet:

```markdown
- **`saga.rs`** — Saga / process-manager primitive, the **dual of an aggregate**. `Saga: Aggregate` supertrait adds `CorrelationKey` (instance routing key; bounds `Clone + Eq + Hash + Send + Sync + Debug + 'static`), `Command: Message` (thin outgoing intent vocabulary — enrichment to a target aggregate's fat command is the runtime's job, not nexus's), and `intent_for(&EventOf<Self>) -> Option<Command>` (Model A: commands are a 1:1 projection of the saga's own events, never a second output, so a dispatch can never drift from a recorded event). `React<E, N>: Saga` is the dual of `Handle<C, N>`: `react(state, &event) -> Result<Option<Events<EventOf, N>>, Error>` consumes an upstream event (the saga's "command" analogue — transient input, never folded) and produces the saga's own events (folded via the existing `AggregateState`). `Option` because a saga legitimately ignores events (`Ok(None)`); two ignore levels: `correlate -> None` (not routed) and `react -> Ok(None)` (routed, no-op). `correlate(&E) -> Option<CorrelationKey>` extracts the routing key (pure; instance *resolution* is the runtime's). One `React` impl per upstream event type. `AggregateRoot::react` dispatches like `AggregateRoot::handle`. No new macro — `#[nexus::aggregate]` supplies the `Aggregate` half. Store-side bounded saga repository (`load → react → save → return intents`) is a follow-up; the runtime loop/cursor/lifecycle is Agency's. See `docs/plans/2026-06-18-saga-primitive-design.md`.
```

- [ ] **Step 2: Note `SagaFixture` in the `testing.rs` bullet**

In the same section, the `testing.rs` bullet currently describes `AggregateFixture`. Append a sentence at the end of that bullet:

```markdown
`SagaFixture<S>` (typestate `SagaFixture → SagaGiven → SagaReacted<N>`) is the saga dual: `given` (replay own history) → `when` (incoming event, drives `AggregateRoot::react` + projects intents via `intent_for`) → `then_expect_events` / `then_expect_commands` / `then_expect_ignored` / `then_expect_error[_matching]` / `then_expect_state`. `correlate` is tested directly, not via the fixture.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document saga primitive + SagaFixture in CLAUDE.md (#127)"
```

---

## Task 5: Final gate + PR

- [ ] **Step 1: Run the full gate once locally via a no-op commit OR rely on the pre-commit hook**

The pre-commit hook already ran `nix flake check` on every commit above. To be explicit before pushing, confirm the working tree is clean:

Run: `git status --short`
Expected: empty (all work committed).

- [ ] **Step 2: Push the branch**

Run: `git push -u origin feat/saga-primitive-127`

- [ ] **Step 3: Open the PR**

```bash
gh pr create --title "feat(kernel): saga / process-manager primitive + SagaFixture (#127)" \
  --body "$(cat <<'EOF'
Implements the nexus half of #127: the pure, callable saga primitive plus its test fixture. Store-side saga repository and all runtime concerns (loop, cursor, lifecycle, dispatch, supervision, timers) are out of scope (Agency / follow-up).

## What

- `Saga: Aggregate` supertrait: `CorrelationKey`, `Command: Message`, `intent_for` (Model A — commands are a 1:1 projection of the saga's own events).
- `React<E, N>: Saga`: dual of `Handle<C, N>`; `react(state, &event) -> Result<Option<Events>, Error>`.
- `AggregateRoot::react` dispatch (dual of `handle`).
- `SagaFixture` given/when/then fixture (feature `testing`), mirroring `AggregateFixture`.
- Tests across the 4 mandatory categories + direct `correlate` tests.

## Design

`docs/plans/2026-06-18-saga-primitive-design.md`. A saga folds only its own events; the upstream event it consumes is the transient input to `react`, exactly as a command is to `handle`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review (completed during authoring)

- **Spec coverage:** `Saga`/`React`/`intent_for`/`correlate` → Task 1. `AggregateRoot::react` → Task 1. `SagaFixture` + all `then_expect_*` → Task 2. 4 mandatory test categories + `correlate` direct test → Task 3. Vocabulary/design rationale → docs in Task 1 + Task 4. Scope split (store repo deferred) → stated in design + PR body.
- **Placeholder scan:** none — all steps contain full code and exact commands.
- **Type consistency:** `react` signature `Result<Option<Events<EventOf<_>, N>>, Error>` identical across `React` trait, `AggregateRoot::react`, and `SagaReacted::result`. `intent_for(&EventOf<Self>) -> Option<Command>` identical in trait and fixture use. Fixture states `SagaFixture`/`SagaGiven`/`SagaReacted` used consistently. Sample domain (`OrderSaga`, `SagaEvent`, `Intent`, `OrderPlaced`, `PaymentSettled`) identical between the in-src unit tests and the integration tests.
- **Known subtlety:** the `#[should_panic]` lifecycle test cannot construct a genuine out-of-sequence history through the public `given` API (it always feeds `1..=n`), so it triggers `replay` failure via a saga with `MAX_REHYDRATION_EVENTS = 1` — the realistic `given`-panic path.
