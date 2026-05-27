//! Tests for the `futures::Stream` bridge introduced in PR2 of the
//! stream refactor.
//!
//! Coverage matrix:
//!
//! 1. **Sequence/Protocol**: `.try_map(...).into_stream().try_collect()`
//!    end-to-end — verifies the bridge yields every item in order with
//!    the owned-value transformation applied by `.try_map`.
//! 2. **Lifecycle**:
//!    - empty lending stream → empty futures stream (terminates on first
//!      poll with `None`).
//!    - many items → terminates after yielding all items.
//!    - after exhaustion, subsequent polls keep returning `None`.
//! 3. **Defensive Boundary**:
//!    - underlying-stream error propagates through `.try_map` into the
//!      bridge: the futures stream yields one `Err(...)` item, then
//!      terminates on the next poll.
//!    - `.try_map` closure error short-circuits similarly.
//! 4. **Send/Spawn**: `IntoStream<...>` is `Send` and can be driven from
//!    a `tokio::spawn` task.
//!
//! Plus a static-assertion that `IntoStream` itself is `Send` — the
//! bridge has to be spawnable for it to be useful.

#![cfg(feature = "futures-bridge")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::missing_docs_in_private_items, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(
    clippy::missing_const_for_fn,
    reason = "tests: Vec args prevent const fn"
)]

use std::convert::Infallible;
use std::fmt;

use futures::StreamExt;
use futures::TryStreamExt;
use nexus::Version;
use nexus_store::PersistedEnvelope;
use nexus_store::store::GlobalSeq;
use nexus_store::stream::{BaseEventStream, EventStream, EventStreamExt, IntoStream};

fn build_persisted(
    version: Version,
    global_seq: GlobalSeq,
    event_type: &str,
    payload: &[u8],
) -> PersistedEnvelope {
    let mut buf = Vec::with_capacity(event_type.len() + payload.len());
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(payload);
    let value = bytes::Bytes::from(buf);
    let et_end = u32::try_from(event_type.len()).expect("event_type fits u32");
    let pl_end = u32::try_from(event_type.len() + payload.len()).expect("payload fits u32");
    PersistedEnvelope::try_new(
        version,
        global_seq,
        value,
        1,
        0..et_end,
        et_end..pl_end,
        None,
    )
    .expect("test fixture envelope")
}

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal lending stream over `(version, event_type, payload)` rows.
/// Identical shape to the `VecStream` in `combinator_tests.rs`.
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
    fn to_envelope<'a>(item: PersistedEnvelope) -> PersistedEnvelope
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for VecStream {
    type Item<'a> = PersistedEnvelope;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(build_persisted(
            Version::new(row.0).unwrap(),
            GlobalSeq::INITIAL,
            &row.1,
            &row.2,
        )))
    }
}

fn rows(n: u64) -> Vec<(u64, String, Vec<u8>)> {
    (1..=n)
        .map(|v| (v, "E".to_string(), vec![v as u8]))
        .collect()
}

/// Lending stream that fails on the `fail_at`-th `next()` call (1-indexed).
/// Used to verify error propagation through the bridge.
struct FailingStream {
    seen: u64,
    fail_at: u64,
}

impl FailingStream {
    const fn new(fail_at: u64) -> Self {
        Self { seen: 0, fail_at }
    }
}

#[derive(Debug)]
struct BoomError;

impl fmt::Display for BoomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("boom")
    }
}

impl std::error::Error for BoomError {}

impl BaseEventStream for FailingStream {
    fn to_envelope<'a>(item: PersistedEnvelope) -> PersistedEnvelope
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for FailingStream {
    type Item<'a> = PersistedEnvelope;
    type Error = BoomError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
        self.seen += 1;
        if self.seen == self.fail_at {
            return Err(BoomError);
        }
        // Synthesise a placeholder envelope when not failing.
        Ok(Some(build_persisted(
            Version::new(self.seen).unwrap(),
            GlobalSeq::INITIAL,
            "E",
            &[0u8; 1],
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Static — IntoStream is Send (must be spawnable)
// ═══════════════════════════════════════════════════════════════════════════

// Anchored on a concrete `.try_map(...)` shape so the assertion is
// meaningful even for the non-trivial bound stack.
fn _assert_into_stream_send() {
    fn is_send<T: Send>() {}
    is_send::<
        IntoStream<
            nexus_store::stream::TryMap<
                VecStream,
                fn(PersistedEnvelope) -> Result<u64, Infallible>,
                Infallible,
            >,
            (),
        >,
    >();
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol — end-to-end try_map -> into_stream -> try_collect
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_map_then_into_stream_then_try_collect_yields_all_items_in_order() {
    let stream = VecStream::new(rows(4))
        .try_map(|env| Ok::<_, Infallible>((env.version(), env.payload().to_vec())))
        .into_stream();

    let collected: Vec<(Version, Vec<u8>)> = stream.try_collect().await.unwrap();
    assert_eq!(collected.len(), 4);
    assert_eq!(
        collected,
        vec![
            (Version::new(1).unwrap(), vec![1]),
            (Version::new(2).unwrap(), vec![2]),
            (Version::new(3).unwrap(), vec![3]),
            (Version::new(4).unwrap(), vec![4]),
        ]
    );
}

#[tokio::test]
async fn map_then_into_stream_yields_owned_closure_outputs() {
    // `.map` (infallible) produces an owned `u64`. The bridge surfaces
    // each value wrapped in `Ok(...)` — there's no per-item error path
    // for the closure, only for the underlying stream.
    let stream = VecStream::new(rows(3))
        .map(|env| env.version().as_u64())
        .into_stream();
    let collected: Vec<Result<u64, Infallible>> = stream.collect().await;
    let values: Vec<u64> = collected.into_iter().map(Result::unwrap).collect();
    assert_eq!(values, vec![1, 2, 3]);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn empty_lending_stream_yields_empty_futures_stream() {
    let stream = VecStream::new(Vec::new())
        .try_map(|env| Ok::<_, Infallible>(env.version()))
        .into_stream();
    let collected: Vec<Version> = stream.try_collect().await.unwrap();
    assert!(collected.is_empty());
}

#[tokio::test]
async fn bridge_is_fused_after_exhaustion() {
    // After the underlying stream returns Ok(None), subsequent polls
    // must keep returning None forever — the futures-stream contract.
    let stream = VecStream::new(rows(2))
        .try_map(|env| Ok::<_, Infallible>(env.version()))
        .into_stream();
    let mut pinned = Box::pin(stream);

    // First two `next()`s yield the items.
    assert_eq!(
        pinned.next().await.unwrap().unwrap(),
        Version::new(1).unwrap()
    );
    assert_eq!(
        pinned.next().await.unwrap().unwrap(),
        Version::new(2).unwrap()
    );

    // Then None ... and stays None.
    assert!(pinned.next().await.is_none());
    assert!(pinned.next().await.is_none());
    assert!(pinned.next().await.is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary — error propagation
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn underlying_stream_error_propagates_through_bridge_then_terminates() {
    // FailingStream errors on its 3rd next() call. The bridge yields
    // 2 Ok items, 1 Err item, then terminates with None.
    let stream = FailingStream::new(3)
        .try_map(|env| Ok::<_, BoomError>(env.version().as_u64()))
        .into_stream();
    let mut pinned = Box::pin(stream);

    assert_eq!(pinned.next().await.unwrap().unwrap(), 1);
    assert_eq!(pinned.next().await.unwrap().unwrap(), 2);

    let err_item = pinned.next().await.unwrap();
    assert!(err_item.is_err(), "expected Err, got {err_item:?}");
    assert_eq!(err_item.unwrap_err().to_string(), "boom");

    assert!(pinned.next().await.is_none());
    assert!(pinned.next().await.is_none());
}

#[derive(Debug)]
struct CloseError;

impl fmt::Display for CloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("close")
    }
}

impl std::error::Error for CloseError {}

impl From<Infallible> for CloseError {
    fn from(_: Infallible) -> Self {
        Self
    }
}

#[tokio::test]
async fn try_map_closure_error_surfaces_through_bridge_then_terminates() {
    // The closure rejects payload byte == 2; the bridge yields one Ok,
    // one Err, then terminates.
    let stream = VecStream::new(rows(4))
        .try_map(|env| {
            if env.payload()[0] == 2 {
                Err(CloseError)
            } else {
                Ok(env.version().as_u64())
            }
        })
        .into_stream();
    let mut pinned = Box::pin(stream);

    assert_eq!(pinned.next().await.unwrap().unwrap(), 1);

    let err_item = pinned.next().await.unwrap();
    assert!(err_item.is_err());
    assert_eq!(err_item.unwrap_err().to_string(), "close");

    assert!(pinned.next().await.is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Spawn — bridge is driveable from a tokio task
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bridge_is_driveable_from_tokio_spawn() {
    let handle = tokio::spawn(async {
        let stream = VecStream::new(rows(5))
            .try_map(|env| Ok::<_, Infallible>(env.version().as_u64()))
            .into_stream();
        stream.try_collect::<Vec<u64>>().await
    });
    let collected = handle.await.unwrap().unwrap();
    assert_eq!(collected, vec![1, 2, 3, 4, 5]);
}
