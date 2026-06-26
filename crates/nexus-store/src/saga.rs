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

use core::fmt;
use core::future::Future;
use core::iter::Chain;
use core::option;

use arrayvec::ArrayVec;
use nexus::{AggregateRoot, DomainEvent, React, Saga, Version};

use crate::error::StoreError;
use crate::repository::{Repository, first_persisted_version};

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
        Self::is_conflict(self)
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
    pub fn iter(
        &self,
    ) -> Chain<option::Iter<'_, ProjectedIntent<S>>, core::slice::Iter<'_, ProjectedIntent<S>>>
    {
        self.first.iter().chain(self.rest.iter())
    }

    /// Number of intents (`0..=N + 1`).
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.rest.len()
    }

    /// `true` when the saga produced events but none projected an intent.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.first.is_none()
    }
}

impl<'a, S: Saga, const N: usize> IntoIterator for &'a ProjectedIntents<S, N> {
    type Item = &'a ProjectedIntent<S>;
    type IntoIter =
        Chain<option::Iter<'a, ProjectedIntent<S>>, core::slice::Iter<'a, ProjectedIntent<S>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<S: Saga, const N: usize> IntoIterator for ProjectedIntents<S, N> {
    type Item = ProjectedIntent<S>;
    type IntoIter =
        Chain<option::IntoIter<ProjectedIntent<S>>, arrayvec::IntoIter<ProjectedIntent<S>, N>>;

    fn into_iter(self) -> Self::IntoIter {
        self.first.into_iter().chain(self.rest)
    }
}

impl<S: Saga, const N: usize> fmt::Debug for ProjectedIntents<S, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

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

/// The saga-facing port: `react → save → project` as one callable bounded
/// transaction.
///
/// Extends [`Repository<S>`] and inherits its snapshot-aware `load` and atomic,
/// optimistic `save` unchanged. Both methods are provided; the blanket impl
/// below gives them to every repository for free.
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
            let Some(produced) = root.react::<E, N>(event).map_err(SagaError::React)? else {
                return Ok(Reaction::Ignored);
            };

            // First version this append assigns. Checked pre-save so overflow is
            // a clean error (save would also reject it).
            let first = first_persisted_version(before).ok_or(SagaError::VersionOverflow)?;

            // Persist atomically (optimistic concurrency enforced inside `save`).
            // `&produced` carries the kernel's `>= 1` guarantee straight into
            // `save` with no Vec materialization; `produced` stays alive as the
            // projection source below (intents are minted only after durability).
            self.save(root, &produced).await.map_err(SagaError::Store)?;

            // Project intents, each pinned to its event's assigned version.
            // `produced` is non-empty (`react` returned `Some`; `Events` holds
            // >= 1), so the loop runs at least once and `current` ends on the
            // last event's version. `peekable` advances the version only when a
            // successor exists, sidestepping a bare `len() - 1` index computation.
            let mut intents = ProjectedIntents::<S, N>::new();
            let mut current = first;
            let mut iter = produced.iter().peekable();
            while let Some(recorded) = iter.next() {
                if let Some(intent) = S::intent_for(recorded) {
                    intents.push(ProjectedIntent::new(root.id().clone(), current, intent));
                }
                if iter.peek().is_some() {
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

#[cfg(test)]
mod error_tests {
    use super::SagaError;
    use crate::error::StoreError;
    use arrayvec::ArrayString;
    use nexus::Version;

    type TestStoreError =
        StoreError<std::io::Error, std::convert::Infallible, std::convert::Infallible>;
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

#[cfg(test)]
mod projected_intents_tests {
    use super::{ProjectedIntent, ProjectedIntents};
    use nexus::{
        Aggregate, AggregateState, DomainEvent, Events, Id, Message, React, Saga, Version,
    };

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
            #[allow(
                clippy::cast_possible_truncation,
                clippy::as_conversions,
                reason = "test: v ranges 1..=3, fits u8"
            )]
            let tag = v as u8;
            intents.push(ProjectedIntent::new(Sid(9), version, Cmd(tag)));
        }
        assert_eq!(intents.len(), 3);
        assert!(!intents.is_empty());
        let versions: Vec<u64> = intents
            .iter()
            .map(|p| p.source_version().as_u64())
            .collect();
        assert_eq!(versions, vec![1, 2, 3]);
        let owned: Vec<u8> = intents.into_iter().map(|p| p.into_intent().0).collect();
        assert_eq!(owned, vec![1, 2, 3]);
    }
}
