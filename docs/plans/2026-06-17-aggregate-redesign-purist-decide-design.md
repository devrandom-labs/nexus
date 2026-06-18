# Aggregate Redesign — Purist Decide on a Marker ("World C")

**Status:** Approved (design). Supersedes the `from_root` / "root↔entity bridge" direction.
**Date:** 2026-06-17
**Branch:** `feat/aggregate-test-fixture` (same branch; this lands before the #126 fixture).
**Tracking issue:** [#197](https://github.com/devrandom-labs/nexus/issues/197) (milestone "Tier 1: Table Stakes"). Blocks/reshapes [#126](https://github.com/devrandom-labs/nexus/issues/126).

## Goal

Collapse the aggregate API to its smallest coherent set and dissolve the
load→decide→save seam, with **extreme compile-time safety and no redundant
concepts**. After this change a freshly loaded `AggregateRoot<A>` is directly
decidable — no wrapper, no bridge.

## Problem

Two disconnected worlds:

- **Persistence world** — `Repository::load -> AggregateRoot<A>`,
  `Repository::save(&mut AggregateRoot<A>, &[EventOf<A>])`.
- **Decision world** — `Handle::handle` lives on the entity newtype
  (`impl Handle<Withdraw> for BankAccount`), never on `AggregateRoot`.

A loaded `AggregateRoot<A>` cannot decide a command. The seam was previously
papered over by hand-rolled stores and by-hand mechanics demos. On top of the
seam, the "aggregate family" carried four nexus concepts plus a user newtype:
`AggregateState`, `Aggregate`, `AggregateRoot`, `AggregateEntity`, and the
macro-generated newtype `BankAccount(AggregateRoot<BankAccount>)`. Two of those
(`AggregateEntity` + the newtype) are mechanical glue that exist only because
`Handle` was placed on the newtype.

## Decision: Purist World C

**The aggregate type becomes a bare marker. `Handle` is implemented on the
marker as a pure decide function of `(state, command) -> events`. A loaded
`AggregateRoot<A>` dispatches to it directly.** The newtype, `AggregateEntity`,
and `from_root` are deleted — not bridged.

```rust
// 1. The aggregate is a marker carrying only the type-level spec.
#[nexus::aggregate(state = AccountState, error = AccountError, id = AccountId)]
struct BankAccount;            // stays a unit struct; macro emits only `impl Aggregate`

// 2. Handle is implemented ON THE MARKER. Decide is a pure function of state+command.
impl Handle<Withdraw> for BankAccount {
    fn handle(state: &AccountState, cmd: Withdraw)
        -> Result<Events<AccountEvent>, AccountError>
    {
        if state.balance < cmd.amount { return Err(AccountError::Insufficient) }
        Ok(events![AccountEvent::Withdrawn(cmd.amount)])
    }
}

// 3. nexus puts one inherent dispatch method on its OWN type — the seam is gone.
impl<A: Aggregate> AggregateRoot<A> {
    pub fn handle<C, const N: usize>(&self, cmd: C)
        -> Result<Events<EventOf<A>, N>, A::Error>
    where A: Handle<C, N>
    { A::handle(self.state(), cmd) }
}
```

The loop — no wrap, no `from_root`, `Repository` signatures unchanged:

```rust
let mut account = repo.load(id).await?;       // AggregateRoot<BankAccount>
let decided = account.handle(Withdraw(50))?;  // decide directly on what load returned
repo.save(&mut account, &decided).await?;
```

### The trait, precisely

```rust
pub trait Handle<C, const N: usize = 0>: Aggregate {
    /// Pure decision: reads domain state, returns decided events or a domain error.
    /// No `&self`, no access to version/identity — a decision is a function of
    /// current state and the command, nothing else.
    fn handle(state: &Self::State, cmd: C)
        -> Result<Events<EventOf<Self>, N>, Self::Error>;
}
```

## Why purist (`&State`), not flexible (`&AggregateRoot<Self>`)

The handler sees **only domain state**. It cannot read `version()` or `id()`.

- **It forbids persistence coupling by construction.** A decision physically
  cannot branch on the stream version/position — the exact property 7 of 8
  surveyed ecosystem crates deliberately enforce.
- **`Handle` is decoupled from `AggregateRoot`.** The trait doesn't mention the
  container at all; a decision is trivially unit-testable from a bare `State`.
- **It pushes events toward carrying deltas, not identity.** The aggregate id is
  the stream key; embedding it in payloads is redundant (CLAUDE.md rule 4).
- The only thing given up — `id` inside decide — is recovered, if ever truly
  needed, by having the command carry it or (future) adding an explicit
  `&Self::Id` parameter. We do **not** expose `version`. Current audit: every
  existing handler reads only `state()`, so nothing needs `id`/`version`.

## Evidence (this was settled empirically, not assumed)

**Coherence — compiled and run on rustc 1.96-nightly, edition 2024:**

| Design | Local command | Foreign command |
|---|---|---|
| `impl Handle for AggregateRoot<Marker>` (World B) | ✅ compiles | ❌ E0117 orphan violation |
| `impl Handle for Marker`, dispatch on `AggregateRoot` (World C) | ✅ compiles + runs | ✅ compiles + runs |

World C coheres unconditionally because `Self` is the local marker. World B has
a real cliff: it silently works until a command type comes from another crate.
The inherent `AggregateRoot::handle` dispatch resolves and executes the correct
impl at runtime (verified). Const-generic note: one `Handle<C, N>` impl per
command type is turbofish-free; two impls differing only in `N` for the same
command are genuinely ambiguous (E0283) — so **one `Handle` per command type**.

**Ecosystem (8 crates surveyed: cqrs-es, esrs, disintegrate, thalo, eventually,
fmodel, eventsourcing, KurrentDB client):** the modern, persistence-serious
crates put decide as a **function of `(&state, command) -> events`** with version
held separate from the value domain logic mutates (validates our
`AggregateRoot`-holds-version split). **Nobody generates a newtype wrapper** —
everyone parameterizes one generic container by the user's plain type. World C
purist puts us on the paved path.

**Usage map (whole workspace):** zero inherent domain methods on any aggregate
newtype (only the generated `new`); zero foreign commands today; every handler
body touches only `state()`; redacted `Debug` already exists on `AggregateRoot`.
Removing the newtype loses nothing real.

## What is deleted

- `AggregateEntity` trait (and its `root`/`root_mut`/`from_root`/delegating
  defaults) — entirely.
- The macro-generated newtype `struct Name(AggregateRoot<Name>)` and its custom
  `Debug` + `new`.
- `from_root` — never added (its reason to exist evaporates).
- The "entity" concept in docs/CLAUDE.md.

`#[nexus::aggregate(state=…, error=…, id=…)]` is simplified to emit **only**
`impl Aggregate for <unit struct>` (the marker keeps being a unit struct). The
`DomainEvent` and `transforms` macros are untouched.

## The family after this change (4 concepts, each load-bearing)

- **`AggregateState`** — state data + evolve (`initial`, `apply(self,&E)->Self`).
- **`Aggregate`** — method-free marker spec (`State`/`Error`/`Id`/`MAX_REHYDRATION_EVENTS`).
- **`AggregateRoot<A>`** — the versioned container + replay machinery + the new
  `handle` dispatch.
- **`Handle<C, N>`** — the pure decide function, implemented on the marker.

## Footgun methods (safety follow-up, lightly addressed here)

`AggregateRoot::advance_version`, `apply_events`, `apply_event`, and `replay`
must only be driven by the repository, never by user code. In World C the normal
user flow never calls them (`load`/`save`/`handle` cover everything). A **hard**
cross-crate seal (so only `nexus-store` can call them while other downstream
crates cannot) is not achievable with plain `pub(crate)` — the repository lives
in a separate crate — and would require a capability-token + sealed-trait dance
or merging the kernel/store boundary (rejected; the boundary is deliberate).

**This change:** mark these methods `#[doc(hidden)]` with explicit "internal —
driven by the repository" docs, shrinking the practical footgun now. **Deferred:**
a hard capability-token seal is filed as a separate issue (its own design; not
allowed to bloat this change).

## Repository / facade — unchanged signatures

`Repository::load -> AggregateRoot<A>` and
`save(&mut AggregateRoot<A>, &[EventOf<A>])` **keep their current shapes.** The
seam dissolves because the *returned* `AggregateRoot<A>` is now directly
decidable — there is no facade change, no `ReplayFrom` change, no `Snapshotting`
change required by this redesign. (They keep working verbatim.)

## Blast radius (all mechanical)

- `crates/nexus/src/aggregate.rs` — delete `AggregateEntity`; reshape `Handle`
  (move to marker, `handle(state, cmd)`); add `AggregateRoot::handle` dispatch;
  `#[doc(hidden)]` the mutators; update the two doc examples.
- `crates/nexus-macros/src/lib.rs` — gut the `aggregate` macro to emit only
  `impl Aggregate`.
- `crates/nexus-macros/tests/expand_roundtrip.rs` — rewrite expected expansion.
- `crates/nexus/tests/kernel_tests/{newtype_aggregate_tests,integration_test,aggregate_root_tests}.rs`
  — convert each aggregate from newtype to marker; move `Handle` impls; update
  handler bodies (`self.state()` → `state`).
- `crates/nexus-macros/tests/cross_crate_test/` — same conversion.
- `examples/inmemory`, `examples/store-and-kernel` — convert; the in-memory
  example can drop its hand-rolled store and `handle` directly on the root.
- `CLAUDE.md` — rewrite the aggregate-family description; remove the
  entity/newtype narrative.

## Impact on #126 (test fixture)

The existing fixture **plan and design are rewritten**, not rebased:
- `from_root` Task is **deleted** (no `from_root` exists).
- The fixture holds an `AggregateRoot<A>` directly: `given` builds
  `AggregateRoot::<A>::new(id)` and replays; `when` calls `root.handle(cmd)`
  (the dispatch); `then_expect_*` unchanged in spirit.
- The sample aggregate becomes a marker, handlers become `handle(state, cmd)`.

## Non-goals (researched, explicitly rejected)

- **Branded/generative lifetimes (GhostCell)** for instance-binding the decided
  events — over-engineering: breaks the async save path and goes viral on
  signatures. Ordinary `&mut self` + events-paired-with-aggregate gives ~90% at
  zero ceremony.
- **Full typestate machine** (Fresh/Loaded/Decided phantom param) — more concept
  surface than it earns; sealing `replay` gives the useful half for free.
- **Facade signature change** (option 2's `load -> entity`) — unnecessary;
  dispatch on the root dissolves the seam without touching the store API.

## Risks / open items

- Toolchain bump to rustc 1.98-nightly (2026-06-16) lands with this work; the
  gate (`nix flake check`) must stay green on the new nightly (validated before
  first commit).
- Const-generic ambiguity: enforce "one `Handle<C, N>` per command type" — the
  multi-event capacity `N` is per command, so this is natural.
