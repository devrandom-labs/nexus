//! The single catch-up-then-live-tail loop, generic over [`Catchup`].
//! Returned as `impl Stream` (RPIT) — no `Box<dyn>`. Holds the only copy of
//! the arm-before-confirm-rescan lost-wakeup discipline, for every adapter.
//!
//! The user-facing [`Subscription`](crate::Subscription) assembles this loop
//! per call site over the [`catchup`](crate::catchup) seam.

use futures::StreamExt;

use crate::PersistedEnvelope;
use crate::catchup::Catchup;

/// Reopen granularity: drop + reopen the bounded scan every `CATCHUP_CHUNK`
/// delivered rows during catch-up, so one adapter scan (and the GC watermark
/// it may pin) is never held across an unbounded backlog.
// pub (not pub(crate)) to satisfy clippy::redundant_pub_crate inside a
// pub(crate) module.
pub const CATCHUP_CHUNK: usize = 1024;

/// Live-loop state threaded through [`futures::stream::unfold`].
struct LiveState<C: Catchup> {
    c: C,
    /// Next INCLUSIVE read position; `None` means the resume position overflowed
    /// (last delivered was the max) so no further event can ever exist.
    read_from: Option<C::Position>,
    /// The currently-open scan, or `None` when one must be opened.
    scan: Option<C::Scan>,
    drained_in_chunk: usize,
}

/// Catch up over the backlog, then tail live forever, as one `impl Stream`.
///
/// The returned stream NEVER yields `None`: once caught up it parks on
/// [`Catchup::arm`] and resumes when a wake lands. Bound it with `take(..)` if
/// a finite prefix is wanted.
///
/// # On error
///
/// Adapter errors — both a failure to open a scan ([`Catchup::read_from`]) and
/// a failing scan item — are surfaced as `Err` stream items. The cursor does
/// **not** terminate on an error and does **not** back off: the next poll
/// reopens a scan from the last *successfully delivered* position. A delivered
/// event is therefore never re-delivered, but a *persistent* error is
/// re-surfaced on every subsequent poll (there is no internal retry budget or
/// dead-letter). Consumers MUST stop consuming on `Err` (e.g. via
/// [`futures::TryStreamExt`]); recovery policy (dead-letter / rebuild) is the
/// consumer's concern. This matches the "never returns `None`" subscription
/// contract — the stream end is the consumer's decision, not the cursor's.
// pub (not pub(crate)) to satisfy clippy::redundant_pub_crate inside a
// pub(crate) module.
pub fn live<C: Catchup + 'static>(
    c: C,
    from: Option<C::Position>,
) -> impl futures::Stream<Item = Result<PersistedEnvelope, C::Error>> + Send
where
    // `StreamExt::next` requires `Unpin`, and the scan is held by-value across
    // awaits in the `unfold` state — so the scan must be `Unpin`. Placed
    // locally on this fn rather than on the `Catchup` trait to avoid
    // over-constraining the seam.
    C::Scan: Unpin,
{
    let state = LiveState {
        read_from: C::next_pos(from),
        c,
        scan: None,
        drained_in_chunk: 0,
    };
    futures::stream::unfold(state, |mut s| async move {
        loop {
            // (1) Ensure an open scan.
            if s.scan.is_none() {
                let Some(rf) = s.read_from else {
                    // Resume position overflowed: no further event can exist.
                    // Park (responds to shutdown when all wake senders drop).
                    s.c.arm().await;
                    continue;
                };
                match s.c.read_from(rf).await {
                    Ok(scan) => {
                        s.scan = Some(scan);
                        s.drained_in_chunk = 0;
                    }
                    Err(e) => return Some((Err(e), s)),
                }
            }

            // (2) Drain one item.
            // Unreachable: phase (1) just guaranteed an open scan. Defensive
            // only — never taken in practice.
            let Some(scan) = s.scan.as_mut() else {
                continue;
            };
            match scan.next().await {
                Some(Ok(env)) => {
                    s.read_from = C::next_pos(Some(C::position_of(&env)));
                    s.drained_in_chunk += 1;
                    if s.drained_in_chunk >= CATCHUP_CHUNK {
                        s.scan = None; // reopen next iteration from the advanced read_from
                    }
                    return Some((Ok(env), s));
                }
                Some(Err(e)) => {
                    s.scan = None;
                    return Some((Err(e), s));
                }
                None => {
                    // (3) Caught up. Arm BEFORE the confirming re-scan (lost-wakeup
                    // discipline), then park only if the re-scan is genuinely empty.
                    s.scan = None;
                    let wait = s.c.arm();
                    let Some(rf) = s.read_from else {
                        wait.await;
                        continue;
                    };
                    match s.c.read_from(rf).await {
                        Ok(mut probe) => match probe.next().await {
                            Some(Ok(env)) => {
                                s.read_from = C::next_pos(Some(C::position_of(&env)));
                                s.scan = Some(probe);
                                s.drained_in_chunk = 1;
                                return Some((Ok(env), s));
                            }
                            Some(Err(e)) => return Some((Err(e), s)),
                            None => {
                                drop(probe);
                                wait.await;
                            }
                        },
                        Err(e) => return Some((Err(e), s)),
                    }
                }
            }
        }
    })
}

#[cfg(all(test, feature = "testing"))]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::shadow_reuse, reason = "test code: env rebinds per loop turn")]
#[allow(
    clippy::shadow_unrelated,
    reason = "test code: env rebinds per loop turn"
)]
#[allow(clippy::doc_markdown, reason = "test code: prose doc comments")]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures::StreamExt;
    use nexus::Version;
    use tokio::time::timeout;

    use super::*;
    use crate::catchup::{AllCatchup, Catchup, StreamCatchup};
    use crate::envelope::pending_envelope;
    use crate::store::{GlobalSeq, RawEventStore};
    use crate::stream_id::StreamKey;
    use crate::testing::InMemoryStore;

    const MUST_DELIVER: Duration = Duration::from_secs(5);

    /// Append events with versions `lo..=hi` to stream `id`.
    async fn seed_range(store: &InMemoryStore, id: &StreamKey, lo: u64, hi: u64) {
        for v in lo..=hi {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(b"e".to_vec())
                .unwrap()
                .build();
            store.append(id, Version::new(v - 1), &[env]).await.unwrap();
        }
    }

    /// Catch-up delivers the full backlog in strict version order.
    #[tokio::test]
    async fn catch_up_yields_backlog_in_order() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed_range(&store, &id, 1, 5).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let versions: Vec<u64> = live(catchup, None)
            .take(5)
            .map(|r| r.unwrap().version().as_u64())
            .collect()
            .await;

        assert_eq!(
            versions,
            vec![1, 2, 3, 4, 5],
            "catch-up must deliver the backlog in order"
        );
    }

    /// After catch-up the cursor parks, then a post-subscribe append wakes it.
    /// This exercises the arm/park lost-wakeup path.
    #[tokio::test]
    async fn live_tail_sees_post_subscribe_append() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed_range(&store, &id, 1, 1).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let cursor = live(catchup, None);
        tokio::pin!(cursor);

        // Drain the single catch-up event.
        let first = timeout(MUST_DELIVER, cursor.next())
            .await
            .expect("catch-up event must arrive")
            .expect("stream never ends")
            .unwrap();
        assert_eq!(first.version().as_u64(), 1, "catch-up event is version 1");

        // Append version 2 after the cursor is parked.
        let writer = Arc::clone(&store);
        let appender = tokio::spawn(async move {
            seed_range(&writer, &StreamKey::from_slice(b"s"), 2, 2).await;
        });

        let second = timeout(MUST_DELIVER, cursor.next())
            .await
            .expect("live append must wake the parked cursor")
            .expect("stream never ends")
            .unwrap();
        assert_eq!(
            second.version().as_u64(),
            2,
            "live tail must deliver the post-subscribe append"
        );
        appender.await.unwrap();
    }

    /// Crossing the chunk-reopen boundary delivers every version exactly once,
    /// in order — no duplicate, no gap. Load-bearing for the reopen logic.
    #[tokio::test]
    async fn chunk_boundary_no_duplicate_no_gap() {
        let total = u64::try_from(CATCHUP_CHUNK).unwrap() + 3;
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed_range(&store, &id, 1, total).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let take_n = CATCHUP_CHUNK + 3;
        let versions: Vec<u64> = live(catchup, None)
            .take(take_n)
            .map(|r| r.unwrap().version().as_u64())
            .collect()
            .await;

        let expected: Vec<u64> = (1..=total).collect();
        assert_eq!(
            versions, expected,
            "chunk-reopen must deliver 1..=total with no duplicate and no gap"
        );
    }

    /// The same loop drives `$all` (GlobalSeq order). Catch-up must interleave
    /// two streams by `global_seq`, and a post-subscribe append to *either*
    /// stream must reach the parked cursor live (the `$all` wake path).
    #[tokio::test]
    async fn all_catchup_yields_global_order_then_live_append() {
        let store = Arc::new(InMemoryStore::new());
        // Interleave across two streams so global_seq spans both: a@1, b@1, a@2.
        seed_range(&store, &StreamKey::from_slice(b"a"), 1, 1).await;
        seed_range(&store, &StreamKey::from_slice(b"b"), 1, 1).await;
        // a@2 builds on a's head (expected version 1).
        let env = pending_envelope(Version::new(2).unwrap())
            .event_type("E")
            .payload(b"e".to_vec())
            .unwrap()
            .build();
        store
            .append(&StreamKey::from_slice(b"a"), Version::new(1), &[env])
            .await
            .unwrap();

        let catchup = AllCatchup::new(Arc::clone(&store)).unwrap();
        let cursor = live(catchup, None);
        tokio::pin!(cursor);

        // Catch-up: ascending global_seq across both streams.
        let mut seqs = Vec::new();
        for _ in 0..3 {
            let env = timeout(MUST_DELIVER, cursor.next())
                .await
                .expect("catch-up event must arrive")
                .expect("stream never ends")
                .unwrap();
            seqs.push(env.global_seq().as_u64());
        }
        assert_eq!(
            seqs,
            vec![1, 2, 3],
            "$all catch-up must deliver every stream's events in GlobalSeq order"
        );

        // Live: append to stream `b` after the cursor parked — must wake it.
        let writer = Arc::clone(&store);
        let appender = tokio::spawn(async move {
            let env = pending_envelope(Version::new(2).unwrap())
                .event_type("E")
                .payload(b"e".to_vec())
                .unwrap()
                .build();
            writer
                .append(&StreamKey::from_slice(b"b"), Version::new(1), &[env])
                .await
                .unwrap();
        });

        let live_env = timeout(MUST_DELIVER, cursor.next())
            .await
            .expect("live append must wake the parked $all cursor")
            .expect("stream never ends")
            .unwrap();
        assert_eq!(
            live_env.global_seq().as_u64(),
            4,
            "$all live tail must deliver the post-subscribe append at global_seq 4"
        );
        appender.await.unwrap();
    }

    // ── Error propagation ────────────────────────────────────────────────────

    /// A test-only error so the mock `Catchup`'s scan can fail. `InMemoryStore`
    /// reads never fail, so the only way to exercise the loop's error path is to
    /// inject a failing dependency at the `Catchup` seam. The SUT under test is
    /// `live`; this is a failing dependency, NOT a reimplementation of the loop.
    #[derive(Debug)]
    struct BoomError;

    impl core::fmt::Display for BoomError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("boom")
        }
    }

    impl std::error::Error for BoomError {}

    /// A `Catchup` whose scan yields one `Ok(env)` then one `Err(BoomError)`.
    /// The scan never exhausts before the `Err`, so `arm` is never reached by
    /// the test; it returns a ready future for completeness. The `Ok` envelope
    /// is a real [`PersistedEnvelope`] read back from an [`InMemoryStore`] (not
    /// fabricated), so only the error is synthetic.
    struct FailingCatchup {
        ok_env: PersistedEnvelope,
    }

    impl Catchup for FailingCatchup {
        type Position = GlobalSeq;
        type Scan = futures::stream::Iter<std::vec::IntoIter<Result<PersistedEnvelope, BoomError>>>;
        type Error = BoomError;

        fn read_from(
            &self,
            _from: GlobalSeq,
        ) -> impl core::future::Future<Output = Result<Self::Scan, Self::Error>> + Send {
            // One Ok then one Err — the loop must surface both, in order.
            let scan = futures::stream::iter(vec![Ok(self.ok_env.clone()), Err(BoomError)]);
            core::future::ready(Ok(scan))
        }

        fn arm(&self) -> impl core::future::Future<Output = ()> + Send + 'static {
            core::future::ready(())
        }

        fn position_of(env: &PersistedEnvelope) -> GlobalSeq {
            env.global_seq()
        }

        fn next_pos(resume: Option<GlobalSeq>) -> Option<GlobalSeq> {
            resume.map_or(Some(GlobalSeq::INITIAL), GlobalSeq::next)
        }
    }

    /// An adapter scan item error is surfaced as an `Err` stream item, in order
    /// after the preceding `Ok`. The loop neither swallows the error nor
    /// terminates the stream on it.
    #[tokio::test]
    async fn scan_item_error_is_surfaced_in_order() {
        // Read back a real PersistedEnvelope to feed the mock's Ok item.
        let store = Arc::new(InMemoryStore::new());
        seed_range(&store, &StreamKey::from_slice(b"s"), 1, 1).await;
        let ok_env = store
            .read_all(GlobalSeq::INITIAL)
            .await
            .unwrap()
            .next()
            .await
            .expect("seeded event must be present")
            .unwrap();

        let cursor = live(FailingCatchup { ok_env }, None);
        tokio::pin!(cursor);

        let first = timeout(MUST_DELIVER, cursor.next())
            .await
            .expect("first item must arrive")
            .expect("stream never ends");
        assert!(first.is_ok(), "first item is the Ok event, got {first:?}");

        let second = timeout(MUST_DELIVER, cursor.next())
            .await
            .expect("error item must arrive")
            .expect("stream never ends");
        assert!(
            second.is_err(),
            "scan error must be surfaced as Err, got {second:?}"
        );
    }
}
