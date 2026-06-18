# Bounded Saga Repository Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a callable, bounded `load → react → save → return-intents` saga repository to `nexus-store` as a thin trait over the existing `Repository`, with version-pinned capability-token intents and zero new persistence machinery.

**Architecture:** Because `Saga: Aggregate`, the existing `EventStore`/`Repository`/`Snapshotting` machinery already persists sagas. This feature adds one extension trait `SagaRepository<S>: Repository<S>` with two **provided** methods (`react_and_save`, `dispatch`) and a blanket impl, so it rides on every repository — bare facade *and* snapshot decorator — for free. Intents are returned as sealed-constructor `ProjectedIntent` tokens (possession proves the event is durable) in a no-alloc `ArrayVec`-backed collection.

**Tech Stack:** Rust 2024 (nightly toolchain), `nexus` kernel (`Saga`/`React`/`AggregateRoot`), `nexus-store` (`Repository`/`StoreError`), `arrayvec`, `thiserror`, `tokio`+`futures` (tests), `InMemoryStore` (testing feature).

**Design doc:** `docs/plans/2026-06-18-saga-repository-design.md`. **Issue:** [#202](https://github.com/devrandom-labs/nexus/issues/202).

**Conventions (from CLAUDE.md / memory):**
- Work on a feature branch (never `main`); PR back. Use the `joeldsouzax` gh account.
- The pre-commit hook runs `nix flake check` automatically — **do not** run the full gate by hand. Run targeted `cargo test`/`cargo clippy` during development.
- `git add` every new source file before any commit (nix flake check fails on untracked modules).
- All imports at the top of the file. No `Box<dyn Error>`. All errors via `thiserror`. No bare arithmetic. Exhaustive enums (no `#[non_exhaustive]`).

---

## Pre-flight

- [ ] **Create the feature branch**

```bash
git checkout -b feat/saga-repository-202
```

---

## Task 1: `first_persisted_version` version helper (shared, arithmetic-safe)

The saga repository must know the first version an append will assign, to pin
each intent to its event's version. That computation already lives inline in
`save_owning`/`save_borrowing`; extract it so both paths and the saga code share
one arithmetic-safe definition.

**Files:**
- Modify: `crates/nexus-store/src/repository.rs` (add helper near `version_to_nz32` at line ~138; refactor the two `match expected_version` sites at lines ~420 and ~712)
- Test: `crates/nexus-store/src/repository.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/nexus-store/src/repository.rs`:

```rust
#[cfg(test)]
mod version_helper_tests {
    use super::first_persisted_version;
    use nexus::Version;

    #[test]
    fn fresh_stream_starts_at_initial() {
        assert_eq!(first_persisted_version(None), Some(Version::INITIAL));
    }

    #[test]
    fn existing_stream_advances_by_one() {
        let v = Version::INITIAL;
        assert_eq!(first_persisted_version(Some(v)), v.next());
    }

    #[test]
    fn overflow_at_max_returns_none() {
        let max = Version::new(u64::MAX).expect("u64::MAX is non-zero");
        assert_eq!(first_persisted_version(Some(max)), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store --lib version_helper_tests`
Expected: FAIL — `cannot find function first_persisted_version`.

- [ ] **Step 3: Add the helper**

In `crates/nexus-store/src/repository.rs`, directly after the `version_to_nz32`
function (around line 143), add:

```rust
/// The first [`Version`] an append will assign, given the stream's current
/// version (`None` = empty stream). Returns `None` on overflow past `u64::MAX`.
///
/// Single source of truth for the "next version to write" computation shared by
/// the aggregate save paths and the saga repository's intent-version pinning —
/// keeps the arithmetic checked in exactly one place (CLAUDE.md rule 2).
pub(crate) const fn first_persisted_version(current: Option<Version>) -> Option<Version> {
    match current {
        None => Some(Version::INITIAL),
        Some(v) => v.next(),
    }
}
```

- [ ] **Step 4: Refactor the two existing call sites to use it**

In `save_owning` (around line 420) replace:

```rust
    let mut next_version = match expected_version {
        None => Version::INITIAL,
        Some(v) => v.next().ok_or(StoreError::VersionOverflow)?,
    };
```

with:

```rust
    let mut next_version =
        first_persisted_version(expected_version).ok_or(StoreError::VersionOverflow)?;
```

Apply the identical replacement in `save_borrowing` (around line 712).

- [ ] **Step 5: Run tests to verify pass + no regression**

Run: `cargo test -p nexus-store --lib version_helper_tests`
Expected: PASS (3 tests).
Run: `cargo test -p nexus-store --test event_store_tests`
Expected: PASS (no regression in the save path).

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-store/src/repository.rs
git commit -m "refactor(store): extract first_persisted_version helper"
```

---

## Task 2: `ConflictPredicate` + `SagaError`

The saga error has two failure domains (react-rejection vs store-failure) plus a
defensive version-overflow guard. `ConflictPredicate` lets `is_conflict()` work
over any repository error without naming a concrete type — `Snapshotting`'s
`Repository::Error` *is* the inner `StoreError`, so one impl covers both.

**Files:**
- Create: `crates/nexus-store/src/saga.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-store/src/saga.rs` with:

```rust
//! Store-side bounded saga repository — the saga analogue of [`Repository`].
//!
//! Because [`Saga`](nexus::Saga) is an [`Aggregate`](nexus::Aggregate), the
//! existing [`Repository`] already loads and saves sagas. This module adds only
//! the saga-specific seam: [`SagaRepository`] (`react → save → project` as one
//! callable bounded transaction), the version-pinned capability-token return
//! types ([`ProjectedIntent`] / [`ProjectedIntents`] / [`Reaction`]), and the
//! two-domain [`SagaError`]. The runtime loop, cursor, correlation *resolution*,
//! conflict *retry*, and intent *dispatch* remain the consumer's (Agency's).
//!
//! See `docs/plans/2026-06-18-saga-repository-design.md`.

use crate::error::StoreError;

mod sealed {
    pub trait Sealed {}
}

/// Predicate over a repository error: is this an optimistic-concurrency
/// conflict (and therefore retryable by reloading + re-reacting)?
///
/// Sealed: implemented inside this crate for [`StoreError`] only. Lets
/// [`SagaError::is_conflict`] delegate without naming a concrete store error —
/// `Snapshotting`'s `Repository::Error` is the inner `StoreError`, so one impl
/// serves bare and snapshotted repositories alike.
pub trait ConflictPredicate: sealed::Sealed {
    /// `true` iff this error is an optimistic-concurrency conflict.
    fn is_conflict(&self) -> bool;
}

impl<A, EncErr, DecErr> sealed::Sealed for StoreError<A, EncErr, DecErr> {}

impl<A, EncErr, DecErr> ConflictPredicate for StoreError<A, EncErr, DecErr> {
    fn is_conflict(&self) -> bool {
        StoreError::is_conflict(self)
    }
}

/// Error from a saga react+persist. Two failure domains plus a defensive
/// overflow guard (CLAUDE.md rule 3 — one variant = one domain).
#[derive(Debug, thiserror::Error)]
pub enum SagaError<SagaErr, StoreErr> {
    /// `react` rejected the upstream event (a saga invariant). Nothing persisted.
    #[error("saga rejected event: {0}")]
    React(#[source] SagaErr),

    /// `load` or `save` failed (adapter / codec / conflict / version overflow).
    #[error(transparent)]
    Store(StoreErr),

    /// Version arithmetic overflowed while pinning intents to event versions.
    /// Defensive: unreachable after a successful `save`, surfaced rather than
    /// panicked (CLAUDE.md rule 2 — no `expect` on data paths).
    #[error("version overflow while projecting saga intents")]
    VersionOverflow,
}

impl<SagaErr, StoreErr: ConflictPredicate> SagaError<SagaErr, StoreErr> {
    /// `true` iff the underlying store error is an optimistic-concurrency
    /// conflict. `React` and `VersionOverflow` are never conflicts (rule 3 —
    /// limit/overflow errors are not retry-eligible conflicts).
    #[must_use]
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Store(e) if e.is_conflict())
    }
}

#[cfg(test)]
mod error_tests {
    use super::SagaError;
    use crate::error::StoreError;
    use arrayvec::ArrayString;
    use nexus::Version;

    type TestStoreError = StoreError<std::io::Error, std::convert::Infallible, std::convert::Infallible>;
    type TestSagaError = SagaError<&'static str, TestStoreError>;

    #[test]
    fn conflict_store_error_is_conflict() {
        let e: TestSagaError = SagaError::Store(StoreError::Conflict {
            stream_id: ArrayString::from("s").expect("fits"),
            expected: Some(Version::INITIAL),
            actual: None,
        });
        assert!(e.is_conflict());
    }

    #[test]
    fn react_error_is_not_conflict() {
        let e: TestSagaError = SagaError::React("rejected");
        assert!(!e.is_conflict());
    }

    #[test]
    fn version_overflow_is_not_conflict() {
        let e: TestSagaError = SagaError::VersionOverflow;
        assert!(!e.is_conflict());
    }
}
```

- [ ] **Step 2: Wire the module into the crate (temporary, so it compiles)**

In `crates/nexus-store/src/lib.rs`, add after the `pub mod repository;` line (line ~93):

```rust
pub mod saga;
```

- [ ] **Step 3: Run test to verify it fails, then passes**

Run: `cargo test -p nexus-store --lib saga::error_tests`
Expected: PASS (3 tests). (The module is new, so it compiles and passes immediately — the test's value is regression protection on the conflict-domain mapping.)

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/saga.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): saga ConflictPredicate + SagaError two-domain error"
```

---

## Task 3: `ProjectedIntent` capability token + `ProjectedIntents` no-alloc collection

`ProjectedIntent` has `pub(crate)` fields and no public constructor — possessing
one proves its saga event is durably persisted. `ProjectedIntents<S, N>` stores
up to `N + 1` of them with **no heap allocation**, using the `Events`
first-plus-rest trick (here `first` is `Option` because some events project no
intent).

**Files:**
- Modify: `crates/nexus-store/src/saga.rs`
- Test: same file

- [ ] **Step 1: Add imports + the two types**

At the top of `crates/nexus-store/src/saga.rs`, extend the imports:

```rust
use core::fmt;
use core::iter::{Chain, once};
use core::option;

use arrayvec::ArrayVec;
use nexus::{Saga, Version};
```

(Keep the existing `use crate::error::StoreError;`.)

Then add, after the `SagaError` block:

```rust
/// One outgoing intent, pinned to the saga-own-event version it projects from.
///
/// **Capability token.** Fields are `pub(crate)` and there is no public
/// constructor: the only way to obtain a `ProjectedIntent` is to receive one
/// from [`SagaRepository::react_and_save`]/[`dispatch`](SagaRepository::dispatch)
/// *after* the append committed. Holding one is a type-level witness that the
/// intent's event is durable — Model A's "never dispatch an unrecorded intent"
/// becomes unrepresentable-otherwise rather than a convention.
pub struct ProjectedIntent<S: Saga> {
    pub(crate) saga_id: S::Id,
    pub(crate) source_version: Version,
    pub(crate) intent: S::Command,
}

impl<S: Saga> ProjectedIntent<S> {
    /// Internal constructor — see the type docs for why this is not public.
    pub(crate) const fn new(saga_id: S::Id, source_version: Version, intent: S::Command) -> Self {
        Self {
            saga_id,
            source_version,
            intent,
        }
    }

    /// `(saga_id, source_version)` — the globally stable, idempotent dedup key
    /// for the runtime's at-least-once outbox. Free under Model A because the
    /// intent *is* a recorded event's projection.
    #[must_use]
    pub const fn dedup_key(&self) -> (&S::Id, Version) {
        (&self.saga_id, self.source_version)
    }

    /// The saga instance this intent belongs to.
    #[must_use]
    pub const fn saga_id(&self) -> &S::Id {
        &self.saga_id
    }

    /// The saga-own-event version this intent projects from.
    #[must_use]
    pub const fn source_version(&self) -> Version {
        self.source_version
    }

    /// Borrow the intent payload.
    #[must_use]
    pub const fn intent(&self) -> &S::Command {
        &self.intent
    }

    /// Consume the token, yielding the bare intent for dispatch.
    #[must_use]
    pub fn into_intent(self) -> S::Command {
        self.intent
    }
}

// Manual Debug: `S` itself is not `Debug`, but `S::Id` (Id: Debug),
// `S::Command` (Message: Debug), and `Version` all are — no extra bounds.
impl<S: Saga> fmt::Debug for ProjectedIntent<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectedIntent")
            .field("saga_id", &self.saga_id)
            .field("source_version", &self.source_version)
            .field("intent", &self.intent)
            .finish()
    }
}

/// A bounded, **heap-free** collection of [`ProjectedIntent`]s — at most
/// `N + 1` (the producing [`Events<_, N>`](nexus::Events) capacity).
///
/// Mirrors `Events`' first-plus-rest layout to hit capacity `N + 1` without the
/// unstable `generic_const_exprs` (`{ N + 1 }`). `first` is `Option` because a
/// saga event may project no intent, so the collection can be empty.
pub struct ProjectedIntents<S: Saga, const N: usize> {
    first: Option<ProjectedIntent<S>>,
    rest: ArrayVec<ProjectedIntent<S>, N>,
}

impl<S: Saga, const N: usize> ProjectedIntents<S, N> {
    pub(crate) const fn new() -> Self {
        Self {
            first: None,
            rest: ArrayVec::new_const(),
        }
    }

    /// Append a token. Total pushes are bounded by the producing event count
    /// (`<= N + 1`) by construction, so the `rest` capacity (`N`) is never
    /// exceeded once `first` absorbs the first push.
    #[allow(
        clippy::expect_used,
        reason = "capacity N+1 is guaranteed by the producing Events<_, N>; overflow is a programmer bug"
    )]
    pub(crate) fn push(&mut self, intent: ProjectedIntent<S>) {
        if self.first.is_none() {
            self.first = Some(intent);
        } else {
            self.rest.try_push(intent).expect(
                "ProjectedIntents capacity exceeded: intents must not exceed the producing Events<_, N> count",
            );
        }
    }

    /// Iterate the tokens in projection order.
    pub fn iter(&self) -> Chain<option::Iter<'_, ProjectedIntent<S>>, core::slice::Iter<'_, ProjectedIntent<S>>> {
        self.first.iter().chain(self.rest.iter())
    }

    /// Number of intents (`0..=N + 1`).
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.rest.len()
    }

    /// `true` when the saga produced events but none projected an intent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }
}

impl<S: Saga, const N: usize> IntoIterator for ProjectedIntents<S, N> {
    type Item = ProjectedIntent<S>;
    type IntoIter = Chain<option::IntoIter<ProjectedIntent<S>>, arrayvec::IntoIter<ProjectedIntent<S>, N>>;

    fn into_iter(self) -> Self::IntoIter {
        self.first.into_iter().chain(self.rest)
    }
}

impl<S: Saga, const N: usize> fmt::Debug for ProjectedIntents<S, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
```

- [ ] **Step 2: Add a focused no-alloc capacity test**

Append to `crates/nexus-store/src/saga.rs`:

```rust
#[cfg(test)]
mod projected_intents_tests {
    use super::{ProjectedIntent, ProjectedIntents};
    use nexus::{Aggregate, AggregateState, DomainEvent, Id, Message, React, Saga, Version};
    use nexus::events::Events;

    // Minimal saga purely to instantiate the generic collection.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct Sid(u8);
    impl core::fmt::Display for Sid {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl AsRef<[u8]> for Sid {
        fn as_ref(&self) -> &[u8] {
            core::slice::from_ref(&self.0)
        }
    }
    impl Id for Sid {
        const BYTE_LEN: usize = 1;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Ev;
    impl Message for Ev {}
    impl DomainEvent for Ev {
        fn name(&self) -> &'static str {
            "Ev"
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Cmd(u8);
    impl Message for Cmd {}

    #[derive(Debug)]
    struct St;
    impl AggregateState for St {
        type Event = Ev;
        fn initial() -> Self {
            Self
        }
        fn apply(self, _e: &Ev) -> Self {
            self
        }
    }

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("err")]
    struct Err;

    struct M;
    impl Aggregate for M {
        type State = St;
        type Error = Err;
        type Id = Sid;
    }
    impl Saga for M {
        type CorrelationKey = u8;
        type Command = Cmd;
        fn intent_for(_e: &Ev) -> Option<Cmd> {
            None
        }
    }
    impl React<Ev> for M {
        fn correlate(_e: &Ev) -> Option<u8> {
            Some(0)
        }
        fn react(_s: &St, _e: &Ev) -> Result<Option<Events<Ev, 0>>, Err> {
            Ok(None)
        }
    }

    #[test]
    fn empty_collection_reports_empty() {
        let intents = ProjectedIntents::<M, 2>::new();
        assert!(intents.is_empty());
        assert_eq!(intents.len(), 0);
        assert_eq!(intents.iter().count(), 0);
    }

    #[test]
    fn holds_n_plus_one_without_panic_and_iterates_in_order() {
        // N = 2 → capacity 3.
        let mut intents = ProjectedIntents::<M, 2>::new();
        for v in 1u64..=3 {
            let version = Version::new(v).expect("non-zero");
            intents.push(ProjectedIntent::new(Sid(9), version, Cmd(v as u8)));
        }
        assert_eq!(intents.len(), 3);
        assert!(!intents.is_empty());
        let versions: Vec<u64> = intents.iter().map(|p| p.source_version().as_u64()).collect();
        assert_eq!(versions, vec![1, 2, 3]);
        let owned: Vec<u8> = intents.into_iter().map(|p| p.into_intent().0).collect();
        assert_eq!(owned, vec![1, 2, 3]);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p nexus-store --lib saga::projected_intents_tests`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/saga.rs
git commit -m "feat(store): ProjectedIntent capability token + no-alloc ProjectedIntents"
```

---

## Task 4: `Reaction` outcome enum

The outcome of one react+persist, mirroring `react`'s `Option`. `#[must_use]`
because dropping it silently discards intents the runtime must dispatch.

**Files:**
- Modify: `crates/nexus-store/src/saga.rs`

- [ ] **Step 1: Add the enum**

After the `ProjectedIntents` block in `crates/nexus-store/src/saga.rs`, add:

```rust
/// Outcome of one [`SagaRepository::react_and_save`]/[`dispatch`](SagaRepository::dispatch).
///
/// `#[must_use]`: discarding it drops intents the runtime was meant to dispatch
/// — a lost-work bug the compiler now warns on (a `Vec` return could not).
#[must_use = "projected intents must be handed to the runtime for dispatch"]
pub enum Reaction<S: Saga, const N: usize> {
    /// `react` returned `Ok(None)` — routed, no-op, nothing persisted.
    Ignored,
    /// `react` produced events; they were appended atomically.
    Reacted {
        /// Version the saga stream advanced to (the last appended event's version).
        version: Version,
        /// Intents projected from the recorded events, in order (`<= one` per event).
        intents: ProjectedIntents<S, N>,
    },
}

impl<S: Saga, const N: usize> fmt::Debug for Reaction<S, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ignored => f.write_str("Ignored"),
            Self::Reacted { version, intents } => f
                .debug_struct("Reacted")
                .field("version", version)
                .field("intents", intents)
                .finish(),
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p nexus-store`
Expected: builds clean (no warnings).

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/saga.rs
git commit -m "feat(store): Reaction outcome enum (must_use)"
```

---

## Task 5: `SagaRepository` trait — `react_and_save` core + `dispatch` + blanket impl

The whole point: two provided methods over `Repository<S>`, blanket-impl'd onto
every repository. `react_and_save` is the irreducible core (root in hand, no
load); `dispatch` is `load` + `react_and_save`.

**Files:**
- Modify: `crates/nexus-store/src/saga.rs`

- [ ] **Step 1: Extend imports**

Update the top-of-file imports in `crates/nexus-store/src/saga.rs` to add:

```rust
use core::future::Future;

use nexus::{AggregateRoot, DomainEvent, EventOf, React};

use crate::repository::{Repository, first_persisted_version};
```

(Merge with the existing `use nexus::{Saga, Version};` — final kernel import line:
`use nexus::{AggregateRoot, DomainEvent, EventOf, React, Saga, Version};`.)

- [ ] **Step 2: Add the trait + blanket impl**

After the `Reaction` block, add:

```rust
/// The saga-facing port: `react → save → project` as one callable bounded
/// transaction. Extends [`Repository<S>`] and inherits its snapshot-aware
/// `load` and atomic, optimistic `save` unchanged. Both methods are provided;
/// the blanket impl below gives them to every repository for free.
pub trait SagaRepository<S: Saga>: Repository<S> {
    /// **Core (single-writer / world A *and* the base for world B).** React to
    /// one upstream `event` against a saga `root` already in hand, persist any
    /// produced own-events atomically, and return their intents pinned to the
    /// versions `save` just assigned. No load — the caller supplies the root.
    ///
    /// - `Ok(Reaction::Ignored)` — `react` returned `Ok(None)`; nothing persisted.
    /// - `Ok(Reaction::Reacted { .. })` — events appended; intents projected.
    /// - `Err(SagaError::React)` — `react` rejected the event; nothing persisted.
    /// - `Err(SagaError::Store)` — load/save failed (use [`SagaError::is_conflict`]).
    ///
    /// # Errors
    /// See the variants above.
    fn react_and_save<E, const N: usize>(
        &self,
        root: &mut AggregateRoot<S>,
        event: &E,
    ) -> impl Future<Output = Result<Reaction<S, N>, SagaError<S::Error, Self::Error>>> + Send
    where
        S: React<E, N>,
        E: DomainEvent,
    {
        async move {
            let before = root.version();

            // React is pure. Ok(None) ⇒ routed but no-op; persist nothing.
            let produced = match root.react::<E, N>(event).map_err(SagaError::React)? {
                None => return Ok(Reaction::Ignored),
                Some(events) => events,
            };

            // Materialize once to satisfy `Repository::save(&[EventOf<S>])`.
            // `save` already allocates a Vec<PendingEnvelope> of equal length
            // internally, so this is a bounded sibling allocation (not new
            // asymptotic cost) and doubles as the projection source below.
            let events: Vec<EventOf<S>> = produced.into_iter().collect();

            // First version this append assigns. Checked pre-save so overflow is
            // a clean error (save would also reject it).
            let first = first_persisted_version(before).ok_or(SagaError::VersionOverflow)?;

            // Persist atomically (optimistic concurrency enforced inside `save`).
            self.save(root, &events).await.map_err(SagaError::Store)?;

            // Project intents, each pinned to its event's assigned version.
            let mut intents = ProjectedIntents::<S, N>::new();
            let mut current = first;
            // `events` is non-empty: `react` returned `Some`, and `Events` holds >= 1.
            let last_index = events.len() - 1;
            for (i, event) in events.iter().enumerate() {
                if let Some(intent) = S::intent_for(event) {
                    intents.push(ProjectedIntent::new(root.id().clone(), current, intent));
                }
                if i < last_index {
                    current = current.next().ok_or(SagaError::VersionOverflow)?;
                }
            }

            Ok(Reaction::Reacted {
                version: current,
                intents,
            })
        }
    }

    /// **Convenience (stateless concurrent reactors / world B).** `load` the
    /// instance then [`react_and_save`](Self::react_and_save). One call per
    /// upstream event; a concurrent writer may cause `save` to surface
    /// `Err(SagaError::Store)` with [`is_conflict`](SagaError::is_conflict) — the
    /// caller reloads and retries. `load` is whichever `Repository<S>::load` is
    /// in play, so snapshot hydration composes for free.
    ///
    /// # Errors
    /// As [`react_and_save`](Self::react_and_save), plus `Err(SagaError::Store)`
    /// from the `load`.
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

// Rides on every repository — bare `EventStore`/`ZeroCopyEventStore` AND the
// `Snapshotting` decorator — with zero per-type code. Fully static dispatch.
impl<S: Saga, R: Repository<S>> SagaRepository<S> for R {}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p nexus-store`
Expected: builds clean.
Run: `cargo clippy -p nexus-store --lib`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/saga.rs
git commit -m "feat(store): SagaRepository trait (react_and_save + dispatch + blanket impl)"
```

---

## Task 6: Public re-exports

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`

- [ ] **Step 1: Add the re-exports**

In `crates/nexus-store/src/lib.rs`, add after the `pub use repository::{...};`
line (line ~124):

```rust
pub use saga::{
    ConflictPredicate, ProjectedIntent, ProjectedIntents, Reaction, SagaError, SagaRepository,
};
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p nexus-store`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "feat(store): re-export saga repository surface"
```

---

## Task 7: Integration tests — the 4 mandatory cross-cutting categories

A real `OrderSaga` (hand-written `Aggregate`+`Saga`+`React`, no derive feature)
driven through `InMemoryStore`, covering sequence/protocol, lifecycle, defensive
boundary, and linearizability/isolation.

**Files:**
- Create: `crates/nexus-store/tests/saga_repository_tests.rs`

- [ ] **Step 1: Write the test file (domain + codec + fixtures)**

Create `crates/nexus-store/tests/saga_repository_tests.rs`:

```rust
//! Bounded saga repository (`SagaRepository`) integration tests — the 4
//! mandatory cross-cutting categories (CLAUDE.md rule 7) over `InMemoryStore`.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::join_all;
use nexus::events::Events;
use nexus::{
    Aggregate, AggregateRoot, AggregateState, DomainEvent, Id, Message, React, Saga, Version,
};
use nexus_store::{
    Decode, Encode, PersistedEnvelope, Reaction, SagaError, SagaRepository, Store,
};
use nexus_store::testing::InMemoryStore;
use tokio::sync::Barrier;

// ── Saga identity ────────────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OrderId(u64);
impl core::fmt::Display for OrderId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl AsRef<[u8]> for OrderId {
    fn as_ref(&self) -> &[u8] {
        // Stable little-endian bytes for the storage key.
        // Leaked-free: we hand back a slice into a thread-local would be
        // overkill; store the bytes in the struct instead.
        unreachable!("use OrderId::key_bytes")
    }
}
```

> **Note for the implementer:** `Id` requires `AsRef<[u8]>` returning a borrow,
> so `OrderId` must *hold* its key bytes. Replace the struct above with the
> byte-holding version below (this is the real definition — the snippet above
> only illustrates why we can't return a slice from a `u64` field):

```rust
// ── Saga identity (real definition) ──────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OrderId([u8; 8]);
impl OrderId {
    fn new(n: u64) -> Self {
        Self(n.to_le_bytes())
    }
    fn as_u64(&self) -> u64 {
        u64::from_le_bytes(self.0)
    }
}
impl core::fmt::Display for OrderId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_u64())
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

// ── The saga's OWN events (its history) ──────────────────────────────────
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

// ── Outgoing intent vocabulary (thin) ────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    TakePayment,
}
impl Message for Intent {}

// ── Saga state ───────────────────────────────────────────────────────────
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

// ── Upstream events the saga CONSUMES ────────────────────────────────────
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

// ── The saga marker ──────────────────────────────────────────────────────
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
            return Ok(None); // duplicate — already handled
        }
        Ok(Some(Events::new(SagaEvent::PaymentRequested)))
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
        Ok(Some(Events::new(SagaEvent::OrderCompleted)))
    }
}

// ── Codec for the saga's OWN events (no `json` feature in the gate) ───────
struct SagaCodec;
impl Encode<SagaEvent> for SagaCodec {
    type Error = Infallible;
    fn encode(&self, event: &SagaEvent) -> Result<Bytes, Self::Error> {
        let byte: u8 = match event {
            SagaEvent::PaymentRequested => 0,
            SagaEvent::OrderCompleted => 1,
        };
        Ok(Bytes::copy_from_slice(&[byte]))
    }
}
impl Decode<SagaEvent> for SagaCodec {
    type Output<'a> = SagaEvent;
    type Error = &'static str;
    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<SagaEvent, Self::Error> {
        match env.payload().first() {
            Some(0) => Ok(SagaEvent::PaymentRequested),
            Some(1) => Ok(SagaEvent::OrderCompleted),
            _ => Err("bad saga event byte"),
        }
    }
}

type Repo = nexus_store::EventStore<InMemoryStore, SagaCodec>;

fn new_repo() -> Repo {
    Store::new(InMemoryStore::new()).repository().codec(SagaCodec).build()
}
```

- [ ] **Step 2: Run to verify the harness compiles (no tests yet)**

Run: `cargo test -p nexus-store --test saga_repository_tests`
Expected: compiles; "running 0 tests". (If `Encode::Error`/`Decode` signatures
differ, fix the codec to match `crates/nexus-store/tests/event_store_tests.rs:85`.)

- [ ] **Step 3: Add Category 1 — Sequence/Protocol**

Append to the test file:

```rust
// ── 1. Sequence/Protocol ─────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_then_reload_then_dispatch_advances_the_saga() {
    let repo = new_repo();
    let id = OrderId::new(1);

    // First upstream event → records PaymentRequested, projects TakePayment.
    let r1: Reaction<OrderSaga, 0> =
        repo.dispatch(id.clone(), &OrderPlaced { id: 1, total: 100 }).await.unwrap();
    match r1 {
        Reaction::Reacted { version, intents } => {
            assert_eq!(version, Version::INITIAL);
            let collected: Vec<_> = intents.iter().collect();
            assert_eq!(collected.len(), 1);
            assert_eq!(collected[0].intent(), &Intent::TakePayment);
            assert_eq!(collected[0].source_version(), Version::INITIAL);
            assert_eq!(collected[0].saga_id(), &id);
        }
        Reaction::Ignored => panic!("expected Reacted"),
    }

    // Second upstream event of a different type → records OrderCompleted,
    // which projects NO intent (internal-only).
    let r2: Reaction<OrderSaga, 0> =
        repo.dispatch(id.clone(), &PaymentSettled { id: 1 }).await.unwrap();
    match r2 {
        Reaction::Reacted { version, intents } => {
            assert_eq!(version, Version::new(2).unwrap());
            assert!(intents.is_empty(), "OrderCompleted projects no intent");
        }
        Reaction::Ignored => panic!("expected Reacted"),
    }

    // Reload from the store: both own-events replayed → terminal state.
    let loaded = repo.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert!(loaded.state().completed);
    assert_eq!(loaded.version(), Version::new(2));
}
```

- [ ] **Step 4: Run Category 1**

Run: `cargo test -p nexus-store --test saga_repository_tests dispatch_then_reload`
Expected: PASS.

- [ ] **Step 5: Add Category 2 — Lifecycle (reopen) + 3 — Defensive boundary**

Append:

```rust
// ── 2. Lifecycle (write → reopen via a fresh facade over the same store) ──

#[tokio::test]
async fn reopen_facade_over_same_store_sees_prior_saga_events() {
    let backend = InMemoryStore::new();
    let store = Store::new(backend);
    let id = OrderId::new(7);

    {
        let repo: Repo = store.repository().codec(SagaCodec).build();
        let _ = repo
            .dispatch::<OrderPlaced, 0>(id.clone(), &OrderPlaced { id: 7, total: 50 })
            .await
            .unwrap();
    }
    // A brand-new facade over the SAME shared store handle.
    let repo2: Repo = store.repository().codec(SagaCodec).build();
    let loaded = repo2.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert_eq!(loaded.version(), Version::INITIAL);
}

// ── 3. Defensive boundary ────────────────────────────────────────────────

#[tokio::test]
async fn react_ok_none_persists_nothing() {
    let repo = new_repo();
    let id = OrderId::new(2);

    // Prime the saga so payment_requested == true.
    let _ = repo
        .dispatch::<OrderPlaced, 0>(id.clone(), &OrderPlaced { id: 2, total: 10 })
        .await
        .unwrap();

    // A duplicate OrderPlaced now reacts to Ok(None) → Ignored, nothing written.
    let r: Reaction<OrderSaga, 0> =
        repo.dispatch(id.clone(), &OrderPlaced { id: 2, total: 10 }).await.unwrap();
    assert!(matches!(r, Reaction::Ignored));

    // Version unchanged from the single recorded event.
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.version(), Version::INITIAL);
}

#[tokio::test]
async fn react_error_rolls_back_nothing() {
    let repo = new_repo();
    let id = OrderId::new(3);

    let err = repo
        .dispatch::<OrderPlaced, 0>(id.clone(), &OrderPlaced { id: 3, total: 0 })
        .await
        .unwrap_err();
    assert!(matches!(err, SagaError::React(OrderSagaError::EmptyOrder)));
    assert!(!err.is_conflict());

    // Nothing persisted: the stream is empty → fresh load at version None.
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn stale_root_save_surfaces_conflict() {
    let repo = new_repo();
    let id = OrderId::new(4);

    // Load a fresh root (version None) and hold it.
    let mut stale = repo.load(id.clone()).await.unwrap();

    // A concurrent writer advances the stream to version 1.
    let _ = repo
        .dispatch::<OrderPlaced, 0>(id.clone(), &OrderPlaced { id: 4, total: 20 })
        .await
        .unwrap();

    // react_and_save against the STALE root expects version None → conflict.
    let err = repo
        .react_and_save::<OrderPlaced, 0>(&mut stale, &OrderPlaced { id: 4, total: 20 })
        .await
        .unwrap_err();
    assert!(err.is_conflict(), "stale expected-version must surface as conflict");
    assert!(matches!(err, SagaError::Store(_)));
}
```

- [ ] **Step 6: Run Categories 2 + 3**

Run: `cargo test -p nexus-store --test saga_repository_tests`
Expected: PASS (all tests so far).

- [ ] **Step 7: Add Category 4 — Linearizability/Isolation (real concurrency)**

Append:

```rust
// ── 4. Linearizability/Isolation ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_dispatch_one_wins_loser_conflicts_then_retry_converges() {
    let repo = Arc::new(new_repo());
    let id = OrderId::new(5);

    // Two reactors race the SAME instance's first event. Both load version None,
    // both react, both try to append at expected version None → exactly one wins.
    let barrier = Arc::new(Barrier::new(2));
    let tasks = (0..2).map(|_| {
        let repo = Arc::clone(&repo);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            let mut root = repo.load(id.clone()).await.unwrap();
            barrier.wait().await; // maximize overlap on the append
            repo.react_and_save::<OrderPlaced, 0>(&mut root, &OrderPlaced { id: 5, total: 30 })
                .await
        })
    });
    let results: Vec<_> = join_all(tasks).await.into_iter().map(|j| j.unwrap()).collect();

    let wins = results.iter().filter(|r| r.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|r| r.as_ref().err().is_some_and(SagaError::is_conflict))
        .count();
    assert_eq!(wins, 1, "exactly one writer commits");
    assert_eq!(conflicts, 1, "the loser surfaces a conflict, not a silent drop");

    // Caller-side retry (reload → re-dispatch) converges: the reload now sees
    // payment_requested == true, so the retry is a clean Ignored no-op.
    let retry: Reaction<OrderSaga, 0> =
        repo.dispatch(id.clone(), &OrderPlaced { id: 5, total: 30 }).await.unwrap();
    assert!(matches!(retry, Reaction::Ignored));

    // Final state: exactly one PaymentRequested recorded.
    let loaded = repo.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert_eq!(loaded.version(), Version::INITIAL);
}
```

- [ ] **Step 8: Run the full test file**

Run: `cargo test -p nexus-store --test saga_repository_tests`
Expected: PASS (all categories).
Run: `cargo clippy -p nexus-store --tests`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/nexus-store/tests/saga_repository_tests.rs
git commit -m "test(store): saga repository — 4 mandatory cross-cutting categories"
```

---

## Task 8: Documentation

**Files:**
- Modify: `CLAUDE.md` (Store Crate section — add the `saga.rs` entry)
- Modify: `docs/plans/2026-06-18-saga-repository-design.md` (flip status)

- [ ] **Step 1: Document the module in CLAUDE.md**

In `CLAUDE.md`, in the `### Store Crate (nexus-store)` file list, add a bullet
after the `repository.rs` entry:

```markdown
- **`saga.rs`** — Store-side bounded saga repository. `SagaRepository<S>: Repository<S>` extension trait, blanket-impl'd onto every repository (bare facade + `Snapshotting` decorator), with two provided methods: `react_and_save(&mut root, event)` (core: react → atomic save → project intents pinned to assigned versions; no load) and `dispatch(id, event)` (load + `react_and_save`, for stateless reactors). Returns `Reaction<S, N>` (`Ignored` | `Reacted { version, intents }`), where `intents` is a no-alloc `ProjectedIntents<S, N>` (ArrayVec, capacity `N+1`, `Events`-style first+rest) of `ProjectedIntent<S>` **capability tokens** (sealed `pub(crate)` constructor — possessing one proves the event is durable; carries `(saga_id, source_version)` dedup key, Model A). `SagaError<SagaErr, StoreErr>` (`React` | `Store` | `VersionOverflow`) with `is_conflict()` via the sealed `ConflictPredicate`. Conflict is **surfaced, never retried internally** (CLAUDE.md rule 5 — no dependency on Agency single-writer). Zero new persistence machinery — reuses `Repository::load`/`save`. See `docs/plans/2026-06-18-saga-repository-design.md`.
```

- [ ] **Step 2: Flip the design-doc status**

In `docs/plans/2026-06-18-saga-repository-design.md`, change the status line:

```markdown
**Status:** implemented (#202)
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md docs/plans/2026-06-18-saga-repository-design.md
git commit -m "docs: document store-side saga repository (saga.rs)"
```

---

## Task 9: Finalize — let the gate run, then PR

- [ ] **Step 1: Confirm all new files are tracked**

Run: `git status --porcelain`
Expected: clean (no untracked `.rs` files — the gate fails on untracked modules).

- [ ] **Step 2: Push and open the PR (the pre-commit hook ran `nix flake check`)**

```bash
git push -u origin feat/saga-repository-202
gh pr create --title "feat(store): bounded saga repository (load → react → save → return intents) (#202)" \
  --body "Implements #202. Thin \`SagaRepository<S>: Repository<S>\` extension trait (blanket-impl'd) adding \`react_and_save\`/\`dispatch\` over the existing repository machinery. Version-pinned capability-token intents, conflict surfaced not retried. Design: docs/plans/2026-06-18-saga-repository-design.md. 4 mandatory test categories covered.

Closes #202."
```

- [ ] **Step 3: Watch CI**

Run: `gh pr checks --watch`
Expected: Nix Flake Check passes.

---

## Self-review notes (verify during execution)

- **Spec coverage:** load→react→save→return-intents (Task 5 `dispatch`); atomic append (inherited `save`); surface-not-retry conflict (Task 5 + Task 7 boundary/linearizability); version-pinned dedup key (Task 3 `ProjectedIntent` + Task 5 pinning loop); snapshot composition (blanket impl over `Repository`, Task 5); 4 mandatory test categories (Task 7). ✅
- **`Encode`/`Decode` signatures:** Task 7's `SagaCodec` mirrors `event_store_tests.rs:85`. If the real `Encode::encode` returns `bytes::Bytes` and `Decode` has `type Output<'a>` + `decode<'a>(&'a self, &'a PersistedEnvelope)` — confirmed against `codec.rs`. Adjust if drift.
- **Type consistency:** `react_and_save`/`dispatch` return `Reaction<S, N>`; `Reaction::Reacted.intents: ProjectedIntents<S, N>`; `ProjectedIntent::new` is `pub(crate)`; `first_persisted_version` is `pub(crate)` in `repository.rs` and imported in `saga.rs`. ✅
- **Arithmetic safety:** all version stepping via `Version::next()` with `ok_or(SagaError::VersionOverflow)`; no bare `+`/`-` except `events.len() - 1` (guarded: `events` non-empty) and `usize::from(bool) + len` (cannot overflow). ✅
- **Allocation:** one justified `Vec<EventOf<S>>` materialization (save allocates internally regardless); return path is no-alloc `ArrayVec`. ✅
```
