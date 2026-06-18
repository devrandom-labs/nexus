# Store-side bounded saga repository — design

**Issue:** [#202](https://github.com/devrandom-labs/nexus/issues/202)
**Follow-up to:** [#127](https://github.com/devrandom-labs/nexus/issues/127) (kernel primitive, delivered in [#201](https://github.com/devrandom-labs/nexus/issues/201))
**Date:** 2026-06-18
**Status:** implemented (#202)
**Crate:** `nexus-store` (the persistence edge; no kernel changes)

## Summary

The kernel saga primitive (`Saga` / `React` / `intent_for` / `correlate`,
`AggregateRoot::react`) shipped pure in #201. #202 is the **store edge**: the
saga analogue of the aggregate `Repository` — a *callable bounded transaction*
that, per resolved saga instance and incoming upstream event, does:

```
load  → replay the saga's own event stream (snapshot-cached where applicable)
react → AggregateRoot::react(state, &event) -> Result<Option<Events>, Error>
save  → append the saga's own events atomically (optimistic concurrency on version)
return→ the projected intents (intent_for over the produced events), in order
```

It is **not** a runtime loop. The cursor, lifecycle, correlation *resolution*,
conflict *retry policy*, and intent *dispatch* remain Agency's, per the #127
boundary.

## The key structural fact: a saga is already an aggregate to the store

Because `Saga: Aggregate`, the existing facade **already persists sagas with no
new code**:

- `EventStore<S, C>` / `ZeroCopyEventStore<S, C>` `impl Repository<A>` for **any**
  `A: Aggregate` — so `repo.load(saga_id)` and `repo.save(&mut root, &events)`
  work for a saga today.
- `AggregateRoot::react` (kernel) already does the react dispatch.
- `Saga::intent_for` (kernel) already does the intent projection.
- The `Snapshotting<EventStore, SS, T>` decorator already gives snapshot-cached
  load with `P = Version`.

So the *entire* load→react→save→project flow is already expressible by composing
pieces that exist:

```rust
// What Agency could already write today, with ZERO additions:
let root = repo.load(saga_id).await?;            // Repository::load (snapshot-aware)
if let Some(events) = root.react::<E, N>(&evt)? { // kernel dispatch
    repo.save(&mut root, &events).await?;         // Repository::save (atomic, optimistic)
    let intents: Vec<_> = events.iter().filter_map(S::intent_for).collect();
    // hand `intents` to Agency for enrichment + dispatch
}
```

**This is the most important finding of the design pass.** #127's "one
persistence model — reuse the existing `Repository` / replay / `SnapshotStore`
machinery" is not an aspiration; it falls out for free. #202 therefore adds only
a thin **convenience + naming** layer, not new persistence machinery.

## What #202 actually adds

Three small things, all justified, none duplicating existing machinery:

1. **A bundled `dispatch` method** so Agency's stateless-handler path makes *one*
   call per upstream event instead of hand-wiring load+react+save+project.
2. **A version-pinned intent return type** so each returned intent carries the
   dedup key Model A makes free (`saga_id` + the saga-event version it projects
   from).
3. **A named port** (`SagaRepository<S>`) Agency can bound on, parallel to
   `Repository<A>`.

### Prior-art grounding (why this shape, not another)

A two-batch primary-source survey (Axon, MassTransit, NServiceBus, Marten;
Commanded, Emmett, Chassaing's Decider, fmodel; Akka attempted) drove three of
the decisions below. Highlights:

- **Closest precedent: Emmett's `Workflow`** (Dudycz) — a pure
  `decide(input, state) -> output` returning the workflow's *own* events plus
  outgoing commands, state event-sourced in its own inbox/outbox stream, runtime
  loop pushed to a separate `workflowProcessor`. This is Nexus's Model A and the
  Nexus↔Agency split, independently arrived at. (Caveat: Emmett's Workflow engine
  is recent/proposal-stage; its *shipped* aggregate `decide`/`evolve` +
  `CommandHandler` split is stable and shows the same pure-step-vs-infra line.)
- **Instructive counter-example: Commanded** — dispatches commands **before**
  persisting process-manager state (`handle → dispatch → apply → persist → ack`),
  and stores PM state as an **overwritten snapshot row**, not an event stream
  (the "event-sourced PM" reading was refuted 0-3 in verification). Model A
  inverts both: **record own events first, then project intents from recorded
  events.** Strictly safer (no dispatch-before-durable window) and the reason the
  dedup key is free.
- **Conflict handling: MassTransit & NServiceBus** both *surface* optimistic
  conflict to the caller and let an external retry pipeline re-drive
  load→react→save; neither retries inside the repository. **Axon** instead
  *prevents* conflict via single-instance pessimistic locking. No surveyed
  library retries internally.
- **Akka single-writer:** the "shard to a single writer so conflict can't
  happen" hypothesis produced **zero** surviving primary-source claims across
  two attempts — it is *unsourced*, neither confirmed nor refuted. The conflict
  choice therefore cannot lean on it.

## The A-vs-B fork *is* the method shape

The unresolved architectural question — does Agency (A) shard each saga instance
to a single writer that holds the root in memory, or (B) run stateless concurrent
reactors that reload per event — maps **directly** onto how the root is supplied:

| World | Who holds the saga root | Wants |
|---|---|---|
| **A** — shard to single writer (actor-per-instance, Akka-style) | the actor, in memory across events | react+save against a root it already has; **no reload** |
| **B** — stateless concurrent reactors | nobody; reload per event | one bundled `load→react→save` call; conflict possible |

The conservative resolution: make the **root-in-hand** operation the irreducible
core (serves world A, and world B builds on it), and offer the **load-bundled**
operation as a convenience (serves world B). Both surface conflict identically;
neither retries.

Crucially, the **root-in-hand core needs nothing new** — it is already
`Repository::save` + kernel `react`/`intent_for` (the snippet above). So #202's
only genuinely new method is the world-B bundled convenience.

## Recommended design (opinionated — built for the crate's north stars)

Every choice below is resolved toward: **compile-time safety over runtime
discipline**, **zero heap allocation on the hot path**, **static dispatch
(monomorphized, inlinable) over vtables**, and **every type justified by an
invariant it carries**. The four open questions are *answered*, not deferred.

### Two methods, one trait, blanket-impl'd — all default methods, zero new persistence code

```rust
/// The saga-facing port: react → save → project, as one callable bounded
/// transaction. Extends `Repository<S>` so it inherits snapshot-aware `load`
/// and atomic, optimistic `save` UNCHANGED. Every method is a default — the
/// blanket impl gives it to every repository, bare facade and `Snapshotting`
/// decorator alike, with no per-type code.
pub trait SagaRepository<S: Saga>: Repository<S> {
    /// CORE (worlds A *and* B). React against a root already in hand, persist
    /// any produced own-events atomically, and project their intents — pinned
    /// to the exact versions just assigned. No load: the single-writer actor
    /// (world A) holds its root across events and never re-reads the store.
    ///
    /// Bundling react+save+project here is the point: you cannot save without
    /// projecting, project without saving, or pin an intent to a version that
    /// was never persisted. Model A's discipline becomes a compile-time path,
    /// not a convention.
    fn react_and_save<E, const N: usize>(
        &self,
        root: &mut AggregateRoot<S>,
        event: &E,
    ) -> impl Future<Output = Result<Reaction<S, N>, SagaError<S::Error, Self::Error>>> + Send
    where
        S: React<E, N>,
        E: DomainEvent;

    /// CONVENIENCE (world B — stateless concurrent reactors). `load` then
    /// `react_and_save`. One call per upstream event; conflict is possible and
    /// surfaces for the caller to retry. Default method — `load` is whichever
    /// `Repository<S>::load` is in play, so snapshots compose for free.
    fn dispatch<E, const N: usize>(
        &self,
        id: S::Id,
        event: &E,
    ) -> impl Future<Output = Result<Reaction<S, N>, SagaError<S::Error, Self::Error>>> + Send
    where
        S: React<E, N>,
        E: DomainEvent,
    {
        async move {
            let mut root = self.load(id).await.map_err(SagaError::Store)?;
            self.react_and_save(&mut root, event).await
        }
    }
}

// Rides on bare facades AND Snapshotting — no per-type impls, static dispatch.
impl<S: Saga, R: Repository<S>> SagaRepository<S> for R {}
```

Generic over `E` and `N` → **monomorphized, no boxing, no vtable**: each
`(saga, upstream-event)` pair compiles to a straight-line call. The `where S:
React<E, N>` bound is discharged at compile time; a saga that doesn't react to
`E` simply has no `dispatch::<E, _>` to call.

### Return type — a capability token, bounded on the stack

```rust
/// One projected intent that PROVES its saga event is durably persisted.
///
/// Fields are `pub(crate)`; there is no public constructor. The ONLY way to
/// hold a `ProjectedIntent` is to receive one from `react_and_save`/`dispatch`
/// AFTER the append committed. Possessing one is a type-level witness that
/// "this intent's event is on disk" — Agency cannot fabricate an intent for an
/// unrecorded event. Model A's "never send without recording" stops being a
/// rule and becomes unrepresentable-otherwise.
pub struct ProjectedIntent<S: Saga> {
    pub(crate) saga_id: S::Id,        // self-contained dedup key, portable across nodes
    pub(crate) source_version: Version,
    pub(crate) intent: S::Command,
}

impl<S: Saga> ProjectedIntent<S> {
    /// `(saga_id, source_version)` — globally stable, idempotent dedup key for
    /// Agency's at-least-once outbox. Free because the intent IS a recorded event.
    pub fn dedup_key(&self) -> (&S::Id, Version) { (&self.saga_id, self.source_version) }
    pub fn intent(&self) -> &S::Command { &self.intent }
    pub fn into_intent(self) -> S::Command { self.intent }
}

/// Outcome of one react+persist. `#[must_use]`: dropping it silently discards
/// intents the runtime was supposed to dispatch — a lost-work bug the compiler
/// now warns on.
#[must_use = "projected intents must be handed to the runtime for dispatch"]
pub enum Reaction<S: Saga, const N: usize> {
    /// `react` returned `Ok(None)` — routed, no-op, nothing persisted.
    Ignored,
    /// `react` produced events; they were appended atomically.
    Reacted {
        /// Version the saga stream advanced to after the append.
        version: Version,
        /// Intents projected from the recorded events, in order (≤ one per
        /// event). `ArrayVec`-backed, capacity `N + 1` — bounded, NO heap
        /// allocation, mirroring `Events<E, N>` exactly.
        intents: ProjectedIntents<S, N>,
    },
}
```

Three deliberate calls, each in service of the crate's values:

1. **`ProjectedIntents<S, N>` is `ArrayVec`-backed, capacity `N + 1`** — not
   `Vec`. Intents are bounded by the `Events<E, N>` capacity that produced them,
   so the bound is *known at compile time*. We already bit the `N + 1`
   const-generic for `Events`; biting it again here keeps the no-alloc promise
   end-to-end. A `Vec` return would be the one heap allocation in an otherwise
   stack-and-`Bytes` pipeline.
2. **`ProjectedIntent` is a capability token** (sealed constructor) — the
   single highest-leverage compile-time-safety move in the whole design. It
   turns Model A's central invariant into something the type system enforces.
3. **`#[must_use]` on `Reaction`** — explicit, idiomatic, and catches the exact
   lost-work bug (drop the intents, lose the dispatch) that a `Vec` return would
   let slide silently.

### Version pinning is arithmetic-safe (rule 2)

Intents are pinned to the versions `save` actually assigned. `react_and_save`
captures the pre-append version and computes the first assigned version via a
shared `pub(crate)` helper (the same `match expected_version` logic
`save_owning` already uses), then walks *forward* `first, first.next(), …` to
pair each event with its version — **never** reconstructing by subtracting from
`root.version()` (which would be unchecked arithmetic on a `NonZeroU64`).

### Error type

`react` can fail with the saga's domain `S::Error` *before* any store
interaction; load/save fail with the repository's `Self::Error`. These are two
failure domains (CLAUDE.md rule 3 — one variant = one domain), so a two-variant
`thiserror` enum (rule: all errors use `thiserror`), allocation-free:

```rust
#[derive(thiserror::Error, Debug)]
pub enum SagaError<SagaErr, StoreErr> {
    /// react() rejected the upstream event (saga invariant). Nothing persisted.
    #[error("saga rejected event: {0}")]
    React(#[source] SagaErr),
    /// load or save failed (adapter / codec / conflict / version overflow).
    #[error(transparent)]
    Store(StoreErr),
}

impl<SagaErr, StoreErr: ConflictPredicate> SagaError<SagaErr, StoreErr> {
    /// Delegates to the inner store error; lets Agency drive `backon`-style
    /// retry without matching variants. `React` is a domain rejection — never a
    /// conflict — so it returns `false` (rule 3: overflow/limit ≠ conflict).
    pub fn is_conflict(&self) -> bool { matches!(self, Self::Store(e) if e.is_conflict()) }
}
```

`ConflictPredicate` is a tiny `pub(crate)`-sealed trait both `StoreError` (which
has `is_conflict()` today, commit 336ed0d) and the snapshot decorator's error
implement — so `SagaError::is_conflict()` works uniformly whether the inner repo
is bare or snapshotted, without naming a concrete store error.

## Conflict handling — surface, never retry internally

`dispatch` surfaces `Conflict` from the inner `save` unchanged; it runs **no**
internal reload-re-react-resave loop. Both architectural camps in the survey
agree the repository is not the retry owner (MassTransit/NServiceBus surface;
Axon prevents). Agency owns the retry policy and already has the predicate:
`StoreError::is_conflict()` (commit 336ed0d), surfaced here via
`SagaDispatchError::is_conflict()`.

This is also the only choice that satisfies **CLAUDE.md rule 5**: relying on an
Agency single-writer guarantee for correctness would be an undocumented
cross-component coupling without structural proof — and the survey could find
*no* primary source proving single-writer removes the conflict (the Akka angle
died in verification twice). So:

- **World B** (concurrent reactors): conflict is real; Agency reloads and retries.
- **World A** (single writer): conflict simply never fires — free defense in
  depth, including the brief two-instances-during-rebalance window that
  actor-per-instance systems are known to expose.

Identical repository contract either way; no dependency on Agency's concurrency
model encoded in nexus.

## Atomicity

One store transaction for the event append (CLAUDE.md rule 1) — inherited
unchanged from `Repository::save` → `save_owning`/`save_borrowing` →
`RawEventStore::append`. Intent **dispatch** is *not* performed here and is *not*
in any transaction with the append: the repository **returns** intents; Agency
dispatches them (at-least-once + dedup on `(saga_id, source_version)`, the
NServiceBus/MassTransit outbox pattern). Recording-before-returning (Model A)
means an intent cannot exist unless its event is already durable — the inverse of
Commanded's dispatch-before-persist window.

## Snapshots / position

Inherited for free. `P = Version` for an own-stream saga via the existing
`Snapshotting<EventStore, SS, T>` decorator and `SnapshotStore<S::State, Version>`.
The blanket `impl<S: Saga, R: Repository<S>> SagaRepository<S>` means `dispatch`
works through the snapshot decorator with no extra code. The `P = GlobalSeq`
pure-reactor cache variant remains an Agency runtime choice (#127), not a fork
here.

## Construction

No new builder. A saga repository *is* a repository:

```rust
let repo = store.repository().codec(OrderSagaCodec).build();       // EventStore<S,C>
// repo.dispatch(saga_id, &upstream_evt).await?  — SagaRepository rides along via blanket impl

let repo = store.repository().codec(c).snapshot_store(ss).build(); // Snapshotting<…> — also works
```

## Testing plan (4 mandatory categories first)

1. **Sequence/Protocol** — `load → dispatch(e1) → dispatch(e2)` against a real
   `InMemoryStore`; reload and assert the saga's own stream + that re-`dispatch`
   of an already-handled event yields `Ignored` (idempotent no-op via state).
2. **Lifecycle** — write saga events, drop the repo, reopen, `dispatch` again and
   assert state continuity; snapshot-hydrate path (`Snapshotting`) reaches the
   same state as full replay.
3. **Defensive boundary** — `Ok(None)` persists nothing (assert version
   unchanged + `Ignored`); `react` `Err` rolls back nothing (assert empty
   stream + `React` error); a stale expected-version `save` returns a
   `Store`-wrapped `Conflict` and `is_conflict()` is `true`; events whose
   `intent_for` is `None` produce `Reacted` with fewer intents than events.
4. **Linearizability/Isolation** — two concurrent `dispatch` calls on the same
   `saga_id` (`tokio::spawn` + `Barrier`): exactly one append wins, the loser
   surfaces `is_conflict()`, and a caller-side reload→re-`dispatch` converges —
   proving world-B retry semantics without any internal loop.

Then the gate: feature-gated tests need a self dev-dependency to run under the
default-feature `nix flake check` (per `project_gate_nextest_default_features`).

## Resolved decisions (the four open questions, answered)

1. **Trait, not inherent method — `SagaRepository<S>: Repository<S>`,
   blanket-impl'd.** Decided *for* the trait: it composes with the `Snapshotting`
   decorator for free (snapshot-cached saga load is in #127 scope; the
   `load_with`/`save_with` inherent precedent deliberately gave up snapshot
   composition, which we cannot afford here), it names the port Agency bounds on,
   and as all-default-methods + one blanket impl it costs *zero* new persistence
   code and stays fully static-dispatch.
2. **Return version-pinned intents, not events.** `Reaction::Reacted { version,
   intents }`. Under Model A the intents *are* the recorded events' 1:1
   projection, and `version` gives the cursor its advance point; the raw events
   carry nothing the `(saga_id, source_version)` token + `version` don't already
   convey. If a future need for the events themselves appears, it's an additive
   field — not a reason to widen the return now (YAGNI, rule 4).
3. **Bite the const generic now — `ProjectedIntents<S, N>` is `ArrayVec`,
   capacity `N + 1`.** A `Vec` would be the lone heap allocation in a
   stack-and-`Bytes` pipeline and would betray the no-alloc ethos the rest of the
   crate pays for. The `Events<E, N>` precedent proves the `N + 1` const generic
   is ergonomic enough in this codebase.
4. **World A: no new method, but the safety is shared, not reimplemented.**
   `react_and_save(&mut root, event)` *is* the world-A primitive (it takes the
   root in hand, no load). World A does not get a *separate* bare projection
   helper — routing it through `react_and_save` is what guarantees its intents
   are capability tokens pinned to real versions. A loose
   `events.iter().filter_map(S::intent_for)` would hand back *un-pinned, forgeable*
   intents and is therefore deliberately **not** offered.

## Remaining genuine unknown (not a design choice — an Agency fact)

Only one thing the survey could not settle, and it is not nexus's to decide:
**does Agency run world A (shard to single writer) or world B (concurrent
reactors)?** The design above is correct in *both* — `react_and_save` is the core
either way, `dispatch` serves B, and conflict surfaces identically. Nexus encodes
no dependency on the answer (rule 5). Agency picks; #202 ships unchanged
regardless.
```
