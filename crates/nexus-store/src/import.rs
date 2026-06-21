//! Import contract — picky, per-stream, halt-not-skip.
//!
//! Import takes already-decoded [`PortableEvent`]s (the *box* — CBOR default
//! or CESR — turns chunk bytes into them; import is box-agnostic) and places
//! them onto caller-supplied target streams. The store's sequential-`append`
//! version check does the hard work; import adds routing, a halt-on-trouble
//! rule, and a per-stream report.
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
//! This module is the contract: the data types ([`PortableEvent`] outcomes,
//! the report, the error) and the [`EventImporter`] trait. The concrete
//! ingest impl is a later card.

use nexus::{Id, Version};
use thiserror::Error;

use crate::portable::PortableEvent;
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

/// Place decoded [`PortableEvent`]s onto caller-routed target streams,
/// picky per stream, halt-not-skip, under an [`Atomicity`] policy.
///
/// `route` maps a producer `stream_id` (the bytes on each [`PortableEvent`])
/// to the receiver's target stream id `I` — this is where origin-namespacing
/// (e.g. `task-123` → `phone:task-123`) happens; import holds no naming
/// policy of its own.
///
/// On success returns an [`ImportReport`] of per-stream outcomes (per-stream
/// atomicity) or completes (whole-chunk). `Err` is reserved for
/// whole-operation failures.
///
/// NOTE: the precise input shape (a fallible stream surfacing per-block
/// checksum failures vs the `&[PortableEvent]` slice here) is finalized in
/// the import-impl card; this is the contract.
pub trait EventImporter: RawEventStore {
    /// Import a chunk's decoded events.
    fn import<I, R>(
        &self,
        events: &[PortableEvent],
        route: R,
        atomicity: Atomicity,
    ) -> impl std::future::Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
    where
        I: Id,
        R: Fn(&[u8]) -> I + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{PendingEnvelope, PersistedEnvelope};
    use crate::error::AppendError;
    use crate::store::GlobalSeq;
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

    // ── EventImporter trait: associated-type / supertrait surface compiles ───
    //
    // No behavioral test — the concrete ingest impl is a later card (import
    // card). This tiny no-op impl exists ONLY to pin the frozen trait surface:
    // it forces `import`'s exact signature (generic `I: Id`, `R: Fn(&[u8]) -> I
    // + Send`, the `Send` future, the `ImportReport`/`ImportError` return
    // shape) and the `RawEventStore` supertrait to compile. The import card
    // will replace this scaffold with the real impl (and its behavioral tests).

    #[derive(Debug, Error)]
    #[error("noop")]
    struct NoopError;

    struct NoopStore;

    impl RawEventStore for NoopStore {
        type Error = NoopError;
        type Stream = futures::stream::Empty<Result<PersistedEnvelope, NoopError>>;
        type AllStream = futures::stream::Empty<Result<PersistedEnvelope, NoopError>>;

        fn append(
            &self,
            _id: &impl Id,
            _expected_version: Option<Version>,
            _envelopes: &[PendingEnvelope],
        ) -> impl std::future::Future<Output = Result<(), AppendError<NoopError>>> + Send {
            std::future::ready(Ok(()))
        }

        fn read_stream(
            &self,
            _id: &impl Id,
            _from: Version,
        ) -> impl std::future::Future<Output = Result<Self::Stream, NoopError>> + Send {
            std::future::ready(Ok(futures::stream::empty()))
        }

        fn read_all(
            &self,
            _from: GlobalSeq,
        ) -> impl std::future::Future<Output = Result<Self::AllStream, NoopError>> + Send {
            std::future::ready(Ok(futures::stream::empty()))
        }
    }

    impl EventImporter for NoopStore {
        fn import<I, R>(
            &self,
            _events: &[PortableEvent],
            _route: R,
            _atomicity: Atomicity,
        ) -> impl std::future::Future<Output = Result<ImportReport<I>, ImportError<Self::Error, I>>> + Send
        where
            I: Id,
            R: Fn(&[u8]) -> I + Send,
        {
            std::future::ready(Ok(ImportReport::new(Vec::new())))
        }
    }

    assert_impl_all!(NoopStore: EventImporter, RawEventStore);
}
