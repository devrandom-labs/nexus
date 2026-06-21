//! Import contract — picky, per-stream, halt-not-skip.
//!
//! Import takes already-decoded events (the *box* — CBOR default or CESR —
//! turns chunk bytes into [`PersistedEnvelope`]s; import is box-agnostic) and
//! places them onto caller-supplied target streams. The store's
//! sequential-`append` version check does the hard work; import adds routing,
//! a halt-on-trouble rule, and a per-stream report.
//!
//! Events carry no per-event stream id (export does no rewrite), so routing
//! is driven by the **per-stream section** the box records: each section names
//! its origin stream once, and import maps that to a target stream via the
//! caller-supplied `route` closure.
//!
//! Resolved semantics (issue #145 §5):
//!
//! - **Picky per stream** — a stream's first incoming version must equal that
//!   stream's next expected version, else that stream is rejected; import
//!   never silently trims a partial overlap.
//! - **Halt, never apply-skip** — a bad block (failed checksum, or version
//!   trouble) halts *its* stream at the last good version and holds back its
//!   later blocks; it never punches a gap.
//! - **Atomicity is a caller policy** ([`Atomicity`]) — whole-chunk
//!   (all-or-nothing, server bulk-restore) vs per-stream (a bad block stops
//!   only its stream, mobile resilience).
//! - **Idempotency is a side-effect** of the version check — re-importing
//!   already-present events is refused, with no dedup machinery.
//!
//! This module is the contract: the data types (the per-stream outcomes,
//! the report, the error) and the [`EventImporter`] trait. The concrete
//! ingest impl is a later card.

use bytes::Bytes;
use nexus::{Id, Version};
use thiserror::Error;

use crate::envelope::{PendingEnvelope, PersistedEnvelope};
use crate::store::RawEventStore;

/// Atomicity granularity for an import — a caller policy, not a format
/// property. The same chunk imports either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atomicity {
    /// All-or-nothing: any bad block rolls back the whole chunk
    /// ([`ImportError::Aborted`]). Best for server bulk-restore on reliable
    /// storage — big transactions, retry the lot.
    WholeChunk,
    /// Each stream's slice commits in its own transaction: a bad block stops
    /// only its stream, the rest commit. Best for mobile resilience on flaky
    /// storage. Reported per stream in an [`ImportReport`].
    PerStream,
}

/// How one stream's import ended, and where the stream sits afterward.
///
/// "Where it sits" lives inside each variant so the success case always
/// carries a real [`Version`] while the trouble cases carry `Option`
/// (`None` = the stream was never touched). Illegal states (a "complete"
/// stream with no version) are unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    /// Every block offered for this stream was applied; it is now at `version`.
    Complete { version: Version },
    /// A block failed its per-block checksum. The good prefix (if any) was
    /// applied, leaving the stream at `reached` (`None` = untouched). The
    /// corrupt block's own version is deliberately absent — a failed checksum
    /// means its decoded header cannot be trusted; re-fetch from `reached`'s
    /// successor.
    Corrupt { reached: Option<Version> },
    /// A block's version did not match the stream's next expected version
    /// (stale overlap or forward gap). Good prefix applied, stream left at
    /// `reached`; `got` is trustworthy (the checksum passed, only the
    /// position was wrong).
    Mismatch {
        reached: Option<Version>,
        got: Version,
    },
}

impl StreamOutcome {
    /// Whether every offered block for this stream was applied.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    /// Where the stream sits after import (`None` = still empty / untouched).
    #[must_use]
    pub const fn reached(&self) -> Option<Version> {
        match self {
            Self::Complete { version } => Some(*version),
            Self::Corrupt { reached } | Self::Mismatch { reached, .. } => *reached,
        }
    }
}

/// One stream's outcome within an import, tagged with the caller's target
/// stream id (echoed verbatim — import owns no naming policy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamReport<I> {
    /// The caller-supplied target stream id this outcome is for.
    pub stream: I,
    /// What happened to it.
    pub outcome: StreamOutcome,
}

/// Per-stream outcomes of an import — one [`StreamReport`] per stream.
///
/// In first-seen order. Describes only work that actually ran (a whole-chunk
/// abort is an [`ImportError`], not a report of "nothing happened").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport<I> {
    streams: Vec<StreamReport<I>>,
}

impl<I> ImportReport<I> {
    /// Build a report from per-stream outcomes.
    #[must_use]
    pub const fn new(streams: Vec<StreamReport<I>>) -> Self {
        Self { streams }
    }

    /// All per-stream outcomes, in first-seen order.
    #[must_use]
    pub fn streams(&self) -> &[StreamReport<I>] {
        &self.streams
    }

    /// The streams the sync loop must act on — everything that isn't
    /// [`StreamOutcome::Complete`].
    pub fn unfinished(&self) -> impl Iterator<Item = &StreamReport<I>> {
        self.streams.iter().filter(|s| !s.outcome.is_complete())
    }

    /// Whether every stream completed.
    #[must_use]
    pub fn all_complete(&self) -> bool {
        self.streams.iter().all(|s| s.outcome.is_complete())
    }
}

/// Why a whole-chunk import aborted — the first bad block's failure mode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AbortReason {
    /// A block failed its per-block checksum.
    #[error("block failed checksum")]
    Corrupt,
    /// A block's version did not match the target stream's next expected
    /// version.
    #[error("version mismatch (expected {expected}, got {got})")]
    Mismatch { expected: Version, got: Version },
}

/// A whole-operation import failure — distinct from the *expected* per-stream
/// outcomes carried in an [`ImportReport`]. `E` is the underlying store
/// error; `I` is the caller's target stream-id type.
#[derive(Debug, Error)]
pub enum ImportError<E, I> {
    /// Chunk header unreadable: bad magic, unknown format-version, or a
    /// structurally unparseable block. Distinct from a per-block checksum
    /// failure (that is a per-stream [`StreamOutcome::Corrupt`]).
    #[error("malformed chunk: {0}")]
    Malformed(&'static str),
    /// Whole-chunk atomicity only: a bad block rolled the entire chunk back —
    /// nothing was written. `stream` is the first offender; retry the whole
    /// chunk. (Per-stream atomicity never aborts — it reports per stream.)
    #[error("chunk aborted at stream {stream:?}: {reason}")]
    Aborted { stream: I, reason: AbortReason },
    /// The underlying store transaction failed.
    #[error(transparent)]
    Store(E),
    /// A stream's `version + 1` overflowed `u64`. NOT a conflict — not
    /// retryable.
    #[error("version overflow")]
    VersionOverflow,
}

/// One origin stream's events, as decoded by the backup box (Card 3).
///
/// The origin stream id is recorded **once** per section (export stamps no
/// per-event id); the importer maps it to a target via the `route` closure.
#[derive(Debug, Clone)]
pub struct StreamSection {
    /// The origin stream id, exactly as the box recorded it.
    pub origin: Bytes,
    /// The section's blocks, in version order as the box laid them down.
    pub blocks: Vec<ImportBlock>,
}

/// One block within a [`StreamSection`].
#[derive(Debug, Clone)]
pub enum ImportBlock {
    /// A block whose per-block checksum passed and decoded to an event.
    Event(PersistedEnvelope),
    /// A block whose per-block checksum **failed**. Carries nothing: a failed
    /// checksum means the decoded header — including its version — cannot be
    /// trusted, which is exactly why [`StreamOutcome::Corrupt`] omits the
    /// version.
    Corrupt,
}

/// One planned per-stream write for [`AtomicAppend::atomic_append_many`].
///
/// `events` is a contiguous, version-preserving run; `expected_version` is the
/// head the target stream must currently be at (`None` = the stream must be
/// fresh). Built by the importer's per-section planner.
#[derive(Debug, Clone)]
pub struct PlannedAppend<I> {
    /// The resolved target stream id.
    pub target: I,
    /// The version the target must currently be at (`None` = fresh stream).
    pub expected_version: Option<Version>,
    /// The contiguous run to append, in version order (always non-empty).
    pub events: Vec<PendingEnvelope>,
}

/// Failure of an [`AtomicAppend::atomic_append_many`] transaction.
///
/// `Conflict` is the cross-stream picky check: write `index`'s
/// `expected_version` did not match the target's actual head (`actual`). The
/// whole transaction is rolled back — nothing landed. `Store` is an
/// adapter-level failure. Distinct domains, distinct variants (CLAUDE rule 3).
#[derive(Debug, Error)]
pub enum AtomicAppendError<E> {
    /// Write at `index` had a head mismatch; `actual` is the target's real head.
    #[error("atomic append conflict at write {index}: actual head {actual:?}")]
    Conflict {
        index: usize,
        actual: Option<Version>,
    },
    /// Adapter-level failure (I/O, encoding, global-seq overflow, …).
    #[error("atomic append store error: {0}")]
    Store(#[source] E),
}

/// Adapter capability: commit several per-stream runs in **one** atomic
/// transaction.
///
/// Either every run lands or none do. This is the primitive
/// [`Atomicity::WholeChunk`] needs and that [`RawEventStore::append`]
/// (per-stream only) cannot provide. Adapters implement it with a real
/// transaction (fjall cross-partition `write_tx`, postgres `BEGIN..COMMIT`,
/// `InMemoryStore` its single mutex).
///
/// # Contract
///
/// - For each write, the target's actual head must equal `expected_version`,
///   else the whole transaction aborts with [`AtomicAppendError::Conflict`]
///   carrying that write's `index` and the target's real head.
/// - Each write's `events` must be a contiguous run starting at
///   `expected_version + 1`. The caller (the importer's planner) guarantees
///   this; implementations validate defensively at their own boundary.
/// - On any failure, **no** write is applied.
pub trait AtomicAppend: RawEventStore {
    /// Append every write atomically. See the trait contract.
    fn atomic_append_many<I: Id>(
        &self,
        writes: &[PlannedAppend<I>],
    ) -> impl std::future::Future<Output = Result<(), AtomicAppendError<Self::Error>>> + Send;
}

/// Place decoded events onto caller-routed target streams, picky per stream,
/// halt-not-skip, under an [`Atomicity`] policy.
///
/// Input is per-stream [`StreamSection`]s carrying [`ImportBlock`]s. Each
/// section's origin stream id (recorded once by the box — export stamps no
/// per-event id) is mapped to the receiver's target stream id `I` by `route`
/// (e.g. `task-123` → `phone:task-123`); import holds no naming policy of its
/// own.
///
/// On success returns an [`ImportReport`] of per-stream outcomes (per-stream
/// atomicity) or completes (whole-chunk). `Err` is reserved for
/// whole-operation failures.
pub trait EventImporter: RawEventStore + AtomicAppend {
    /// Import per-stream sections onto caller-routed target streams.
    fn import<I, R>(
        &self,
        sections: &[StreamSection],
        route: R,
        atomicity: Atomicity,
    ) -> impl std::future::Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
    where
        I: Id,
        R: Fn(&[u8]) -> I + Send;
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code asserts exact values"
)]
mod tests {
    use super::*;
    use crate::envelope::{PendingEnvelope, pending_envelope};
    use bytes::Bytes;
    use futures::StreamExt;
    use static_assertions::assert_impl_all;
    use std::error::Error as _;

    fn v(n: u64) -> Version {
        Version::new(n).expect("test version must be nonzero")
    }

    fn report(stream: &str, outcome: StreamOutcome) -> StreamReport<String> {
        StreamReport {
            stream: stream.to_owned(),
            outcome,
        }
    }

    // ── Send + Sync: report/outcome/error types travel between tasks ─────────

    assert_impl_all!(StreamOutcome: Send, Sync, Copy, Clone);
    assert_impl_all!(Atomicity: Send, Sync, Copy, Clone);
    assert_impl_all!(AbortReason: Send, Sync, Clone);
    assert_impl_all!(StreamReport<String>: Send, Sync, Clone);
    assert_impl_all!(ImportReport<String>: Send, Sync, Clone);
    assert_impl_all!(ImportError<std::io::Error, String>: Send, Sync);

    // ── Atomicity: equality + Copy semantics ────────────────────────────────

    #[test]
    fn atomicity_equality_and_copy_semantics() {
        let policy = Atomicity::WholeChunk;
        let copied = policy; // `Copy`: `policy` is still usable below.
        assert_eq!(policy, copied);
        assert_eq!(Atomicity::WholeChunk, Atomicity::WholeChunk);
        assert_eq!(Atomicity::PerStream, Atomicity::PerStream);
        assert_ne!(Atomicity::WholeChunk, Atomicity::PerStream);
    }

    // ── StreamOutcome::is_complete / reached — all variants ─────────────────

    #[test]
    fn complete_is_complete_and_reaches_its_version() {
        let outcome = StreamOutcome::Complete { version: v(5) };
        assert!(outcome.is_complete());
        assert_eq!(outcome.reached(), Some(v(5)));
    }

    #[test]
    fn corrupt_untouched_is_not_complete_and_reaches_none() {
        let outcome = StreamOutcome::Corrupt { reached: None };
        assert!(!outcome.is_complete());
        assert_eq!(outcome.reached(), None);
    }

    #[test]
    fn corrupt_with_prefix_reaches_that_prefix() {
        let outcome = StreamOutcome::Corrupt {
            reached: Some(v(3)),
        };
        assert!(!outcome.is_complete());
        assert_eq!(outcome.reached(), Some(v(3)));
    }

    #[test]
    fn mismatch_untouched_reaches_none_not_got() {
        // reached and got are distinct values: a bug returning `got` would
        // make this fail.
        let outcome = StreamOutcome::Mismatch {
            reached: None,
            got: v(8),
        };
        assert!(!outcome.is_complete());
        assert_eq!(outcome.reached(), None);
    }

    #[test]
    fn mismatch_with_prefix_reaches_prefix_not_got() {
        let outcome = StreamOutcome::Mismatch {
            reached: Some(v(3)),
            got: v(8),
        };
        assert!(!outcome.is_complete());
        // Must be the prefix (3), never the offending version (8).
        assert_eq!(outcome.reached(), Some(v(3)));
    }

    // ── ImportReport / StreamReport: new / streams / unfinished / all_complete

    #[test]
    fn empty_report_is_vacuously_all_complete_with_no_unfinished() {
        let report: ImportReport<String> = ImportReport::new(Vec::new());
        assert!(report.streams().is_empty());
        assert!(report.all_complete());
        assert_eq!(report.unfinished().count(), 0);
    }

    #[test]
    fn all_complete_report_reports_no_unfinished() {
        let streams = vec![
            report("a", StreamOutcome::Complete { version: v(1) }),
            report("b", StreamOutcome::Complete { version: v(2) }),
        ];
        let report = ImportReport::new(streams.clone());
        assert_eq!(report.streams(), streams.as_slice());
        assert!(report.all_complete());
        assert_eq!(report.unfinished().count(), 0);
    }

    #[test]
    fn mixed_report_filters_exactly_the_non_complete_streams() {
        let streams = vec![
            report("done-1", StreamOutcome::Complete { version: v(1) }),
            report(
                "corrupt",
                StreamOutcome::Corrupt {
                    reached: Some(v(2)),
                },
            ),
            report("done-2", StreamOutcome::Complete { version: v(3) }),
            report(
                "mismatch",
                StreamOutcome::Mismatch {
                    reached: None,
                    got: v(9),
                },
            ),
        ];
        let report = ImportReport::new(streams);

        // streams() preserves first-seen order and full set.
        let all_ids: Vec<&str> = report.streams().iter().map(|s| s.stream.as_str()).collect();
        assert_eq!(all_ids, ["done-1", "corrupt", "done-2", "mismatch"]);

        // unfinished() is EXACTLY the two non-Complete streams, in order.
        let unfinished_ids: Vec<&str> = report.unfinished().map(|s| s.stream.as_str()).collect();
        assert_eq!(unfinished_ids, ["corrupt", "mismatch"]);

        assert!(!report.all_complete());
    }

    #[test]
    fn single_incomplete_stream_blocks_all_complete() {
        let report = ImportReport::new(vec![report(
            "only",
            StreamOutcome::Corrupt { reached: None },
        )]);
        assert!(!report.all_complete());
        assert_eq!(report.unfinished().count(), 1);
    }

    // ── AbortReason / ImportError Display + source ──────────────────────────

    #[test]
    fn abort_reason_display_strings_are_exact() {
        assert_eq!(AbortReason::Corrupt.to_string(), "block failed checksum");
        assert_eq!(
            AbortReason::Mismatch {
                expected: v(4),
                got: v(7),
            }
            .to_string(),
            "version mismatch (expected 4, got 7)",
        );
    }

    #[test]
    fn import_error_malformed_display_is_exact() {
        let err: ImportError<std::io::Error, String> = ImportError::Malformed("bad magic");
        assert_eq!(err.to_string(), "malformed chunk: bad magic");
    }

    #[test]
    fn import_error_aborted_display_formats_stream_debug_and_reason() {
        let err: ImportError<std::io::Error, String> = ImportError::Aborted {
            stream: "phone:task-123".to_owned(),
            reason: AbortReason::Mismatch {
                expected: v(2),
                got: v(5),
            },
        };
        // {stream:?} → quoted Debug of String; reason via its Display.
        assert_eq!(
            err.to_string(),
            "chunk aborted at stream \"phone:task-123\": version mismatch (expected 2, got 5)",
        );
    }

    #[test]
    fn import_error_version_overflow_display_is_exact() {
        let err: ImportError<std::io::Error, String> = ImportError::VersionOverflow;
        assert_eq!(err.to_string(), "version overflow");
    }

    #[test]
    fn import_error_store_is_transparent_forwarding_display_and_source() {
        // A store error WITH its own source, to prove transparent forwarding
        // reaches through to the inner error's source (not just Display).
        #[derive(Debug, Error)]
        #[error("root cause")]
        struct RootCause;

        #[derive(Debug, Error)]
        #[error("store failed")]
        struct DummyStore(#[source] RootCause);

        let err: ImportError<DummyStore, String> = ImportError::Store(DummyStore(RootCause));

        // transparent → Display equals the inner store error's Display.
        assert_eq!(err.to_string(), "store failed");
        // transparent → source() forwards to the inner error's own source.
        let source = err
            .source()
            .expect("transparent Store must forward a source");
        assert_eq!(source.to_string(), "root cause");
    }

    #[test]
    fn non_store_variants_expose_no_error_source() {
        // Error-chain shape pin: `Store` is the ONLY source-forwarding variant.
        // `Aborted` deliberately interpolates its `reason` into the message
        // (not `#[source]`), so it terminates the chain too.
        let malformed: ImportError<std::io::Error, String> = ImportError::Malformed("x");
        assert!(malformed.source().is_none());

        let overflow: ImportError<std::io::Error, String> = ImportError::VersionOverflow;
        assert!(overflow.source().is_none());

        let aborted: ImportError<std::io::Error, String> = ImportError::Aborted {
            stream: "s".to_owned(),
            reason: AbortReason::Corrupt,
        };
        assert!(aborted.source().is_none());
    }

    // ── AtomicAppend helpers and tests ───────────────────────────────────────

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }
    fn tid(s: &str) -> Tid {
        Tid(s.to_owned())
    }

    fn pending(ver: u64, payload: &[u8]) -> PendingEnvelope {
        pending_envelope(v(ver))
            .event_type("E")
            .payload(Bytes::copy_from_slice(payload))
            .expect("valid payload")
            .build()
    }

    fn planned(target: &str, expected: Option<u64>, versions: &[u64]) -> PlannedAppend<Tid> {
        PlannedAppend {
            target: tid(target),
            expected_version: expected.and_then(Version::new),
            events: versions.iter().map(|n| pending(*n, b"p")).collect(),
        }
    }

    async fn head_len(store: &crate::testing::InMemoryStore, id: &Tid) -> usize {
        store
            .read_stream(id, Version::INITIAL)
            .await
            .expect("read opens")
            .filter_map(|r| async move { r.ok() })
            .count()
            .await
    }

    #[tokio::test]
    async fn atomic_append_many_commits_all_writes() {
        use crate::import::AtomicAppend;
        let store = crate::testing::InMemoryStore::new();
        let writes = vec![planned("a", None, &[1, 2]), planned("b", None, &[1])];
        store.atomic_append_many(&writes).await.expect("commits");
        assert_eq!(head_len(&store, &tid("a")).await, 2);
        assert_eq!(head_len(&store, &tid("b")).await, 1);
    }

    #[tokio::test]
    async fn atomic_append_many_rolls_back_all_on_one_conflict() {
        use crate::import::AtomicAppend;
        let store = crate::testing::InMemoryStore::new();
        // Pre-seed "b" to v1 so the second write (expecting fresh) conflicts.
        store
            .append(&tid("b"), None, &[pending(1, b"seed")])
            .await
            .expect("seed");

        let writes = vec![
            planned("a", None, &[1, 2]), // would be fine alone
            planned("b", None, &[1]),    // conflicts: "b" is already at v1
        ];
        let err = store
            .atomic_append_many(&writes)
            .await
            .expect_err("must conflict");
        match err {
            AtomicAppendError::Conflict { index, actual } => {
                assert_eq!(index, 1);
                assert_eq!(actual, Version::new(1));
            }
            AtomicAppendError::Store(_) => panic!("expected Conflict, got Store"),
        }
        // Atomicity: NOTHING landed — "a" must still be empty.
        assert_eq!(head_len(&store, &tid("a")).await, 0, "rolled back");
        assert_eq!(head_len(&store, &tid("b")).await, 1, "unchanged");
    }
}
