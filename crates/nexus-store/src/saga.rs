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
