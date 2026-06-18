# Saga / process-manager primitive — design

**Issue:** [#127](https://github.com/devrandom-labs/nexus/issues/127)
**Date:** 2026-06-18
**Status:** approved, pre-implementation
**Scope of this PR:** pure kernel primitive + `SagaFixture`. Store-side saga
repository is a follow-up PR (tracked under #127).

## Summary

A saga (process manager) coordinates a multi-step process across aggregates. It
is the **dual of an aggregate**:

| | input (NOT folded) | decide | output (folded into own state) |
|---|---|---|---|
| Aggregate | a **command** | `handle` | its own **events** |
| Saga | an upstream **event** | `react` | its own **events** |

The command an aggregate consumes is never folded into aggregate state — it is
transient input; only the resulting events fold. **The upstream event plays the
exact role for a saga that the command plays for an aggregate.** A saga therefore
folds its *own* events (one stream), identically to an aggregate. It does **not**
fold the events it listens to — those are the transient inputs to `react`.

This means a saga is **an aggregate plus three things**:

1. `correlate(event) -> Option<CorrelationKey>` — which running instance an
   incoming event belongs to (pure key *extraction*; instance *resolution* is
   Agency's).
2. `React<E, N>` instead of `Handle<C, N>` — `react(state, event) -> Option<events>`.
3. `intent_for(own_event) -> Option<Command>` — derive an outgoing intent from
   each of the saga's own events.

Everything else (`State`, `Error`, `Id`, the `AggregateState` fold,
`AggregateRoot`, `replay`, version tracking, snapshots) is reused unchanged.

## Vocabulary

There is **no third concept** beyond what nexus already has. Only:

- **Events** (`DomainEvent`) — accepted history.
- **Commands / intents** — requests, not history.

The saga's "own events" are ordinary `DomainEvent`s in the saga's own stream. We
distinguish two event *directions* in prose only — the type system keeps them
apart:

- **upstream events** — produced by other aggregates; the saga *consumes* them
  (the `E` in `React<E>`). Transient input.
- **the saga's own events** — produced by the saga's `react`; the saga *folds*
  them (`EventOf<Self>`). Its history.

The word "fact" is deliberately avoided to prevent the impression of a third type.

## Design decisions

### Commands are derived from events (Model A, 1:1)

`react` returns only the saga's own events. A separate pure projection
`intent_for(own_event) -> Option<Command>` maps each own-event to at most one
outgoing intent. Chosen over emitting `(events, commands)` as two outputs because:

- It preserves the aggregate duality the whole design rests on.
- A send can never drift from a record: every dispatched intent is *derived*
  from a recorded event, so the saga can never record-without-sending or
  send-without-recording.
- Commands stay transient/derived and are never accidentally journaled as
  history (a command can be rejected; an event cannot).

Discipline this imposes: the saga's own events and its outgoing intents must
correspond 1:1, so that `intent_for` stays a dumb, total lookup with **no
branching** — all branching already happened in `react` when it chose *which*
event to record. This matches the project's preference for invariants enforced
by design.

### The Fat Command boundary (why the saga's `Command` is thin)

`handle`/`react` are pure — no services injected. An aggregate's command is
therefore a **Fat Command**: pre-enriched with everything (including data fetched
from third-party services) needed to decide. The enrichment happened earlier,
outside the pure core.

A saga cannot call services either. So a saga's emitted `Command` is a **thin
intent** (`"take payment for order 42"`), *not* the fat command a target
aggregate consumes. Between them sits an enrichment step (call services, load
data, build the fat command) that does IO and therefore lives in **Agency, not
nexus**. Consequently the saga's `Command` type in nexus is just the saga's own
intent vocabulary — a plain `Message`, not bound to any target aggregate's
command type. Agency maps and enriches it downstream.

### `Option` on `react`

`react` returns `Result<Option<Events<EventOf<Self>, N>>, Error>`. Unlike
`handle` (which always produces ≥1 event — a command that decides nothing is an
error), a saga legitimately produces **nothing** for an event it does not care
about. There are two distinct levels of "ignore":

- `correlate(event) -> None` — the event is not routed to any instance at all.
- `react(...) -> Ok(None)` — routed to an instance, but this instance does
  nothing (e.g. a duplicate, or a step already passed).

Both are real and both are kept.

## Trait shape (kernel)

```rust
/// Saga-level contract. A saga IS an aggregate whose events are its own history,
/// plus: how to route incoming events, and how each own-event maps to an intent.
pub trait Saga: Aggregate {
    /// Which running instance an incoming event belongs to. Agency resolves
    /// key -> instance; nexus only extracts the key.
    /// Bounds: Clone + Eq + Hash + Send + Sync + Debug + 'static.
    type CorrelationKey: Clone + Eq + core::hash::Hash + Send + Sync + core::fmt::Debug + 'static;

    /// The saga's emitted intent vocabulary — thin requests, enriched by Agency.
    type Command: Message;

    /// The 1:1 projection: each own-event implies at most one intent.
    /// Dumb, total, pure — all branching already happened in `react`.
    fn intent_for(event: &EventOf<Self>) -> Option<Self::Command>;
}

/// The dual of `Handle<C, N>`: decide from (state, incoming event).
/// One impl per upstream event type the saga consumes (like multiple `Handle`).
pub trait React<E: DomainEvent, const N: usize = 0>: Saga {
    /// Pure correlation-key extraction for this incoming event type.
    fn correlate(event: &E) -> Option<Self::CorrelationKey>;

    /// React to an incoming upstream event.
    /// `Ok(None)`        = not interested / no-op (records nothing).
    /// `Ok(Some(events))` = the saga's own events (>=1), which advance state via `apply`.
    ///
    /// Takes `&E` (borrow) so events read from a stream are not cloned, mirroring
    /// `AggregateRoot::replay`. (`handle` takes the command by value because the
    /// caller constructs it fresh; upstream events are read from elsewhere.)
    fn react(state: &Self::State, event: &E)
        -> Result<Option<Events<EventOf<Self>, N>>, Self::Error>;
}
```

### `AggregateRoot::react` dispatch

Mirroring the existing `AggregateRoot::handle`, add a dispatch method so a loaded
root is directly reactable:

```rust
impl<A: Aggregate> AggregateRoot<A> {
    pub fn react<E, const N: usize>(&self, event: &E)
        -> Result<Option<Events<EventOf<A>, N>>, A::Error>
    where
        A: React<E, N>,
    {
        A::react(self.state(), event)
    }
}
```

### No new macro

Because `Saga: Aggregate`, a user gets the `Aggregate` half from the existing
`#[nexus::aggregate(...)]`, then hand-writes `impl Saga` + `impl React`.
`intent_for` is domain logic a macro cannot generate, so a `#[nexus::saga]` macro
would buy almost nothing. Skipped.

## `SagaFixture` (kernel, `testing` feature)

Mirrors `AggregateFixture` exactly: drives the **real** replay + react path, no
store/codec/serialization. Typestate `SagaFixture<S> → Given<S> → Reacted<S, N>`
makes illegal orderings non-compiling.

```rust
SagaFixture::<OrderSaga>::with_id(id)
    .given([/* the saga's OWN past events */])   // AggregateRoot::replay, v1..=n
    .when(OrderPlaced { total: 150 })            // AggregateRoot::react
    .then_expect_events([PaymentRequested])      // own events react produced
    .then_expect_commands([TakePayment])         // intent_for projected from those
    .then_expect_state(|s| assert!(s.payment_in_flight));
```

- `given(history)` replays the saga's own events via `AggregateRoot::replay`
  (strict version validation, versions `1..=n`) — identical to `AggregateFixture`.
- `when(event)` dispatches through `AggregateRoot::react`, folds any produced
  own-events into the root via the no-clone `apply_events` (so state assertions
  need no `Clone` bound), and runs `intent_for` over each produced event in order
  to collect the projected commands.

Assertions on `Reacted<S, N>` (all chainable, return `Self`):

| assertion | bound | checks |
|---|---|---|
| `then_expect_events([..])` | `EventOf<S>: PartialEq + Debug` | own events `react` produced |
| `then_expect_commands([..])` | `Command: PartialEq + Debug` | intents `intent_for` projected |
| `then_expect_ignored()` | — | `react` returned `Ok(None)` |
| `then_expect_error(e)` | `Error: PartialEq` | `react` returned `Err(e)` |
| `then_expect_error_matching(p)` | — | `react` returned `Err` satisfying `p` |
| `then_expect_state(\|s\| ..)` | — | resulting state (also on `Given`) |

`correlate` is **not** in the fixture flow — in a test the author already knows
which instance they are driving (the `given` history *is* that instance).
Correlation is tested directly: `assert_eq!(OrderSaga::correlate(&event), Some(key))`.

## Out of scope (Agency cards, per issue #127)

- Saga runner loop + cursor/checkpoint (agency#148)
- Saga instance lifecycle & correlation *resolution* (agency#149)
- Command dispatch + enrichment from sagas (agency#150, needs command bus #128)
- Saga error/retry/supervision policy (agency#151)
- Saga deadlines/timers (agency#152)

## Out of scope (this PR; follow-up under #127)

- Store-side bounded saga repository (`load → react → save own events → return
  projected intents`). Touches `nexus-store`; lands as a separate PR so the
  kernel primitive lands clean on its own.

## Persistence note (informational; not built here)

"Pure reactor (no durable own-stream)" vs "own-stream saga" is **one** model with
one variable — *where the journal lives* — set by whether the saga records events
that are re-derivable from upstream (cache with `P = GlobalSeq`) or not (own
stream, `P = Version`). Both are built from the same two pure functions defined
here (`react` + `apply`); which path a given saga uses is an Agency runtime
choice, not a fork in nexus. Already expressed by `SnapshotStore<S, P>` being
generic over `P`.

## Testing plan

The 4 mandatory cross-cutting categories first:

1. **Sequence/Protocol** — multi-step `given(history).when(e1)...` chains; replay
   then react; `react` producing events then folding then a second `react`.
2. **Lifecycle** — replay a saga's own history to a known state, then react;
   version validation on `given` (out-of-sequence history panics in the fixture,
   as with `AggregateFixture`).
3. **Defensive boundary** — `react` returning `Ok(None)` (ignored) vs `Err` vs
   `Ok(Some)`; `intent_for` returning `None` for internal-only events;
   `correlate` returning `None`.
4. **Linearizability/Isolation** — N/A at the pure-primitive layer (no shared
   state, no concurrency); the runtime concurrency lives in Agency.

Then the fixture's own tests run under the default-feature `nix flake check` via
a self dev-dependency (`nexus = { path = ".", features = ["testing", "derive"] }`),
the same pattern `AggregateFixture` uses.
