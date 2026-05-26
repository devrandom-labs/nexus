//! Tests for the lending stream combinators
//! ([`Map`](nexus_store::stream::Map),
//! [`TryMap`](nexus_store::stream::TryMap),
//! [`TryScan`](nexus_store::stream::TryScan))
//! introduced in PR1 of the stream refactor.
//!
//! Coverage matrix (matches PR1 plan):
//!
//! 1. **Sequence/Protocol**: chain `.map().try_map().try_fold(...)` and
//!    verify the transformations apply in order.
//! 2. **Lifecycle**: each combinator over an empty stream completes
//!    naturally and yields no items; over a single item; over many.
//! 3. **Defensive Boundary**:
//!    - `try_map` closure errors mid-stream — error propagates, fold
//!      stops at the failing item.
//!    - underlying stream errors propagate through every combinator.
//!    - empty stream yields no items through any combinator.
//! 4. **Borrowing**: `try_scan` carries a buffer state; yielded items
//!    borrow from the state, and `try_fold` over the scan composes
//!    cleanly. The GAT enforces "one borrow at a time" — verified by
//!    the test's natural shape (chained `try_fold`).

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::missing_docs_in_private_items, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::str_to_string, reason = "tests")]

use std::convert::Infallible;

use nexus::Version;
use nexus_store::PersistedEnvelope;
use nexus_store::store::GlobalSeq;
use nexus_store::stream::{BaseEventStream, EventStream, EventStreamExt};

// ═══════════════════════════════════════════════════════════════════════════
// Test fixture — minimal in-memory lending stream
// ═══════════════════════════════════════════════════════════════════════════

/// Lending stream over `(version, event_type, payload)` rows. Used by every
/// combinator test below.
struct VecStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
}

impl VecStream {
    fn new(rows: Vec<(u64, String, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl BaseEventStream for VecStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for VecStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).unwrap(),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

fn rows(n: u64) -> Vec<(u64, String, Vec<u8>)> {
    (1..=n)
        .map(|v| (v, "E".to_string(), vec![v as u8]))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol — chained combinators apply in order
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn map_then_try_fold_applies_transform_in_order() {
    // .map(env -> u64) materializes each version, then try_fold collects.
    let mut stream = VecStream::new(rows(4)).map(|env| env.version().as_u64());
    let collected: Result<Vec<u64>, Infallible> = stream
        .try_fold(Vec::new(), |mut acc, v| {
            acc.push(v);
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn try_map_then_try_fold_propagates_owned_values() {
    // .try_map produces owned u32; .try_fold sums them. Verifies the
    // owning-output bound and that closure errors short-circuit.
    let mut stream =
        VecStream::new(rows(5)).try_map(|env| Ok::<_, Infallible>(u32::from(env.payload()[0])));
    let sum: Result<u32, Infallible> = stream.try_fold(0u32, |acc, v| Ok(acc + v)).await;
    assert_eq!(sum.unwrap(), 1 + 2 + 3 + 4 + 5);
}

#[tokio::test]
async fn chained_map_and_try_map_preserve_order() {
    // .map(env -> version) .try_map(v -> Ok(v * 10)) — verifies the
    // GAT chain composes and both transforms run in order per item.
    let mut stream = VecStream::new(rows(3))
        .map(|env| env.version().as_u64())
        .try_map(|v| Ok::<_, Infallible>(v * 10));
    let collected: Result<Vec<u64>, Infallible> = stream
        .try_fold(Vec::new(), |mut acc, v| {
            acc.push(v);
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![10, 20, 30]);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle — empty / single / many through every combinator
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn map_over_empty_stream_yields_nothing() {
    let mut stream = VecStream::new(vec![]).map(|env| env.version().as_u64());
    let n: Result<usize, Infallible> = stream.try_count().await.map_err(|i| match i {});
    assert_eq!(n.unwrap(), 0);
}

#[tokio::test]
async fn try_map_over_empty_stream_yields_nothing() {
    let mut stream =
        VecStream::new(vec![]).try_map(|env| Ok::<_, Infallible>(env.version().as_u64()));
    let n: Result<usize, Infallible> = stream.try_count().await;
    assert_eq!(n.unwrap(), 0);
}

#[tokio::test]
async fn try_scan_over_empty_stream_yields_nothing() {
    let mut stream =
        VecStream::new(vec![]).try_scan::<_, _, [u8], Infallible>(Vec::<u8>::new(), |buf, env| {
            buf.clear();
            buf.extend_from_slice(env.payload());
            Ok(buf.as_slice())
        });
    let n: Result<usize, Infallible> = stream.try_count().await;
    assert_eq!(n.unwrap(), 0);
}

#[tokio::test]
async fn try_map_single_event_yields_one() {
    let mut stream = VecStream::new(rows(1)).try_map(|env| Ok::<_, Infallible>(env.payload()[0]));
    let collected: Result<Vec<u8>, Infallible> = stream
        .try_fold(Vec::new(), |mut acc, b| {
            acc.push(b);
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![1]);
}

#[tokio::test]
async fn map_completes_after_many_events() {
    let n = 64;
    let mut stream = VecStream::new(rows(n as u64)).map(|env| env.version().as_u64());
    let count: Result<usize, Infallible> = stream.try_count().await.map_err(|i| match i {});
    assert_eq!(count.unwrap(), n);
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary — closure errors / stream errors / empty
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, thiserror::Error)]
#[error("rejected at version {0}")]
struct Reject(u64);
impl From<Infallible> for Reject {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

#[tokio::test]
async fn try_map_closure_error_short_circuits() {
    // Closure errors on the 3rd item; the fold sees items 1, 2, then Err.
    let mut stream = VecStream::new(rows(5)).try_map(|env| {
        let v = env.version().as_u64();
        if v == 3 { Err(Reject(v)) } else { Ok(v) }
    });

    let mut seen = Vec::new();
    let result: Result<(), Reject> = stream
        .try_for_each(|v| {
            seen.push(v);
            Ok(())
        })
        .await;

    assert!(matches!(result, Err(Reject(3))));
    assert_eq!(seen, vec![1, 2], "items before the failing one are visible");
}

#[tokio::test]
async fn try_scan_closure_error_short_circuits() {
    // Scan rejects when version == 3; fold sees items 1, 2 before the error.
    let mut stream =
        VecStream::new(rows(5)).try_scan::<_, _, [u8], Reject>(Vec::<u8>::new(), |buf, env| {
            if env.version().as_u64() == 3 {
                Err(Reject(3))
            } else {
                buf.clear();
                buf.extend_from_slice(env.payload());
                Ok(buf.as_slice())
            }
        });

    let mut seen = Vec::new();
    let result: Result<(), Reject> = stream
        .try_for_each(|bytes| {
            seen.push(bytes.to_vec());
            Ok(())
        })
        .await;

    assert!(matches!(result, Err(Reject(3))));
    assert_eq!(seen, vec![vec![1u8], vec![2u8]]);
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Borrowing — try_scan yields refs into State; GAT enforces one-at-a-time
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_scan_yields_borrows_from_state_buffer() {
    // The scan buffers each payload into a Vec<u8> field and yields a
    // &[u8] borrowing from it. The GAT projection ties the borrow to the
    // scan's &mut self, so subsequent next() calls invalidate the prior
    // borrow — this test only inspects each borrow within its scope
    // (via try_for_each), demonstrating the borrowing-output use case.
    let mut stream =
        VecStream::new(rows(4)).try_scan::<_, _, [u8], Infallible>(Vec::<u8>::new(), |buf, env| {
            buf.clear();
            buf.extend_from_slice(env.payload());
            Ok(buf.as_slice())
        });

    let mut lengths = Vec::new();
    let result: Result<(), Infallible> = stream
        .try_for_each(|bytes| {
            // Read the borrowed bytes; the GAT guarantees this slice is
            // valid only for the duration of this closure call.
            lengths.push(bytes.len());
            Ok(())
        })
        .await;
    result.unwrap();
    assert_eq!(lengths, vec![1, 1, 1, 1]);
}

#[tokio::test]
async fn try_scan_state_accumulates_across_iterations() {
    // The State field persists across next() calls — verifying that
    // try_scan is genuinely stateful, not just per-item.
    let mut stream =
        VecStream::new(rows(5)).try_scan::<_, _, [u8], Infallible>(Vec::<u8>::new(), |buf, env| {
            // Append each payload to the running buffer.
            buf.extend_from_slice(env.payload());
            Ok(buf.as_slice())
        });

    // After all 5 events, the buffer should contain [1, 2, 3, 4, 5].
    // We snapshot the *last* yielded slice's length via try_fold:
    // since the buffer grows monotonically, the last item's slice
    // length equals the total event count.
    let last_len: Result<usize, Infallible> =
        stream.try_fold(0usize, |_, bytes| Ok(bytes.len())).await;
    assert_eq!(last_len.unwrap(), 5);
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. map_err — error-type conversion combinator
// ═══════════════════════════════════════════════════════════════════════════

/// Fixture stream that fails on the Nth next() call. Used to exercise the
/// error path of `map_err` without needing a real adapter.
struct FailingStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    fail_at: usize,
    pos: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("upstream failed at index {0}")]
struct UpstreamError(usize);

impl BaseEventStream for FailingStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for FailingStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = UpstreamError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos == self.fail_at {
            return Err(UpstreamError(self.pos));
        }
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).unwrap(),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

/// Downstream sink error — represents the kind of error the user wants
/// after `map_err` converts the stream's raw error.
#[derive(Debug, thiserror::Error)]
enum SinkError {
    #[error("wrapped upstream: {0}")]
    Wrapped(#[source] UpstreamError),
}

#[tokio::test]
async fn map_err_passes_items_through_unchanged() {
    // map_err leaves the Item projection alone — only the error type
    // changes. `fail_at: usize::MAX` ensures the stream completes naturally.
    let mut stream = FailingStream {
        rows: rows(3),
        fail_at: usize::MAX,
        pos: 0,
    }
    .map_err(SinkError::Wrapped);
    let collected: Result<Vec<u64>, SinkError> = stream
        .try_fold(Vec::new(), |mut acc, env| {
            acc.push(env.version().as_u64());
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![1, 2, 3]);
}

#[tokio::test]
async fn map_err_over_empty_stream_yields_nothing() {
    let mut stream = FailingStream {
        rows: vec![],
        fail_at: usize::MAX,
        pos: 0,
    }
    .map_err(SinkError::Wrapped);
    let n: Result<usize, SinkError> = stream.try_count().await;
    assert_eq!(n.unwrap(), 0);
}

#[tokio::test]
async fn map_err_converts_upstream_error_to_sink_type() {
    // Stream errors flow through the converter; the sink sees the wrapped form.
    let mut stream = FailingStream {
        rows: rows(5),
        fail_at: 2,
        pos: 0,
    }
    .map_err(SinkError::Wrapped);

    let result: Result<Vec<u64>, SinkError> = stream
        .try_fold(Vec::new(), |mut acc, env| {
            acc.push(env.version().as_u64());
            Ok(acc)
        })
        .await;

    assert!(matches!(result, Err(SinkError::Wrapped(UpstreamError(2)))));
}

#[tokio::test]
async fn map_err_identity_converter_preserves_error_payload() {
    // Identity converter — error round-trips through map_err with no
    // information loss. This is the smoke test for the closure
    // wiring itself.
    let mut stream = FailingStream {
        rows: rows(3),
        fail_at: 0,
        pos: 0,
    }
    .map_err(|e: UpstreamError| e);

    let result: Result<usize, UpstreamError> = stream.try_count().await;
    assert!(matches!(result, Err(UpstreamError(0))));
}

#[tokio::test]
async fn map_err_then_try_map_unblocks_coherence_wall() {
    // This is the structural pattern PR3 wanted but couldn't write:
    // .map_err(convert) .try_map(decode) — the converter eats the
    // adapter error before try_map's `E: From<Self::Error>` bound asks
    // for one. With map_err, Self::Error is already the sink type,
    // and `E: From<E>` is the reflexive impl.
    let mut stream = FailingStream {
        rows: rows(4),
        fail_at: usize::MAX,
        pos: 0,
    }
    .map_err(SinkError::Wrapped)
    .try_map(|env| Ok::<_, SinkError>(env.version().as_u64()));

    let collected: Result<Vec<u64>, SinkError> = stream
        .try_fold(Vec::new(), |mut acc, v| {
            acc.push(v);
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![1, 2, 3, 4]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-consistency: count == collected.len() through each combinator
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn map_count_matches_collected_length() {
    let n = 7;
    let count: Result<usize, Infallible> = VecStream::new(rows(n as u64))
        .map(|env| env.version().as_u64())
        .try_count()
        .await
        .map_err(|i| match i {});
    let collected: Result<Vec<u64>, Infallible> = VecStream::new(rows(n as u64))
        .map(|env| env.version().as_u64())
        .try_collect_map(|v| Ok::<u64, Infallible>(v))
        .await;
    assert_eq!(count.unwrap(), collected.unwrap().len());
}
