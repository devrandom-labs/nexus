//! Zero-infrastructure aggregate test fixture (`given`/`when`/`then`).
//!
//! Drives the real rehydration path: [`given`](AggregateFixture::given)
//! replays a history through [`AggregateRoot::replay`] (strict version
//! validation), then [`when`](Given::when) calls [`Handle::handle`].
//! No store, codec, or serialization. See
//! `docs/plans/2026-06-17-aggregate-test-fixture-design.md`.

use crate::aggregate::{Aggregate, AggregateRoot, EventOf, Handle};
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
        Self {
            id: A::Id::default(),
        }
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
    pub const fn with_id(id: A::Id) -> Self {
        Self { id }
    }

    /// Replay a history onto a fresh aggregate, producing a [`Given`].
    ///
    /// Drives [`AggregateRoot::replay`] with versions `1..=n`.
    ///
    /// # Panics
    ///
    /// Panics if replaying any history event fails — e.g. an out-of-sequence
    /// version surfaced as a [`KernelError`](crate::KernelError). A malformed
    /// history is a test-author setup bug, so failing loudly here is correct.
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
    /// Issue a command. Calls [`AggregateRoot::handle`] (which dispatches to
    /// [`Handle::handle`]), and — on success — folds the decided events into
    /// the root via [`AggregateRoot::apply_events`] so the root reaches the
    /// post-command state, exactly as the repository does after a successful
    /// persist. The fold uses the kernel's `mem::replace` path (no clone), so
    /// the fixture imposes no `Clone` bound on `A::State`. The raw `Result`
    /// is captured for `then_expect_events` / `then_expect_error`. On a
    /// rejected command the root is left at the rehydrated state.
    #[must_use]
    pub fn when<C, const N: usize>(self, cmd: C) -> Acted<A, N>
    where
        A: Handle<C, N>,
    {
        let mut root = self.root;
        let result = root.handle(cmd);
        if let Ok(events) = &result {
            root.apply_events(events);
        }
        Acted { root, result }
    }

    /// Assert against the rehydrated state (no command issued). Returns
    /// `Self` so you can chain another assertion or issue a command with
    /// [`when`](Self::when). Terminal use drops the result — test code
    /// silences `unused_must_use` for exactly this case.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&A::State)) -> Self {
        assertion(self.root.state());
        self
    }
}

/// State after issuing a command. Holds the (already-folded) root and the
/// raw handle result.
pub struct Acted<A: Aggregate, const N: usize> {
    root: AggregateRoot<A>,
    result: Result<Events<EventOf<A>, N>, <A as Aggregate>::Error>,
}

impl<A: Aggregate, const N: usize> Acted<A, N> {
    /// Assert the command succeeded and produced exactly `expected`
    /// (order- and count-sensitive). Returns `Self` for chaining.
    ///
    /// # Panics
    ///
    /// Panics if the command was rejected, or if the decided events do not
    /// match `expected` exactly (same order, same count).
    #[must_use]
    pub fn then_expect_events(self, expected: impl IntoIterator<Item = EventOf<A>>) -> Self
    where
        EventOf<A>: PartialEq + Debug,
    {
        let expected_events: Vec<EventOf<A>> = expected.into_iter().collect();
        assert!(
            self.result.is_ok(),
            "then_expect_events: command was rejected with error: {:?}",
            self.result.as_ref().err()
        );
        let Ok(produced) = &self.result else {
            return self; // unreachable: the assert above diverges on Err
        };
        let produced_refs: Vec<&EventOf<A>> = produced.iter().collect();
        let expected_refs: Vec<&EventOf<A>> = expected_events.iter().collect();
        assert_eq!(
            produced_refs, expected_refs,
            "then_expect_events: decided events did not match expected"
        );
        self
    }

    /// Assert the command was rejected with exactly `expected`. Returns
    /// `Self` for chaining.
    ///
    /// # Panics
    ///
    /// Panics if the command succeeded, or if the error does not equal
    /// `expected`.
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "by-value `expected` is the documented assertion ergonomics (mirrors assert_eq! call style); PartialEq comparison only borrows, so it is not consumed, but requiring &Error at call sites would be worse DX"
    )]
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
        assert_eq!(
            *actual, expected,
            "then_expect_error: error did not match expected"
        );
        self
    }

    /// Assert the command was rejected with an error satisfying `predicate`.
    /// No `PartialEq` bound — for errors wrapping non-comparable sources.
    /// Returns `Self` for chaining.
    ///
    /// # Panics
    ///
    /// Panics if the command succeeded, or if the error does not satisfy
    /// `predicate`.
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

    /// Assert against the resulting state. [`when`](Given::when) already
    /// folded the decided events into the root (on success) via the kernel's
    /// no-clone `apply_events`, so this is a pure borrow of `root.state()` —
    /// no `Clone` bound. After a rejected command nothing was folded and
    /// `handle` does not mutate, so the asserted state equals the rehydrated
    /// state ("the rejected command changed nothing"). Returns `Self` for
    /// chaining.
    #[must_use]
    pub fn then_expect_state(self, assertion: impl FnOnce(&A::State)) -> Self {
        assertion(self.root.state());
        self
    }
}
