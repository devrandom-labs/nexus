//! The single catch-up-then-live-tail loop, generic over [`Catchup`].
//! Returned as `impl Stream` (RPIT) — no `Box<dyn>`. Holds the only copy of
//! the arm-before-confirm-rescan lost-wakeup discipline, for every adapter.
//!
//! This module is additive: the loop is defined here but not yet wired into the
//! public [`Subscription`](crate::Subscription) (a follow-up task). Until then
//! it has no non-test in-crate caller — hence the module-wide `dead_code` allow,
//! matching the [`catchup`](crate::catchup) seam it consumes.
#![allow(
    dead_code,
    reason = "additive live loop not yet wired into the public Subscription"
)]

use futures::StreamExt;

use crate::PersistedEnvelope;
use crate::catchup::Catchup;

/// Reopen granularity: drop + reopen the bounded scan every `CATCHUP_CHUNK`
/// delivered rows during catch-up, so one adapter scan (and the GC watermark
/// it may pin) is never held across an unbounded backlog.
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
pub fn live<C: Catchup + 'static>(
    c: C,
    from: Option<C::Position>,
) -> impl futures::Stream<Item = Result<PersistedEnvelope, C::Error>> + Send
where
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
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures::StreamExt;
    use nexus::Version;
    use tokio::time::timeout;

    use super::*;
    use crate::catchup::{OwnedSubId, StreamCatchup};
    use crate::envelope::pending_envelope;
    use crate::store::RawEventStore;
    use crate::testing::InMemoryStore;

    const MUST_DELIVER: Duration = Duration::from_secs(5);

    /// Append events with versions `lo..=hi` to stream `id`.
    async fn seed_range(store: &InMemoryStore, id: &OwnedSubId, lo: u64, hi: u64) {
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
        let id = OwnedSubId::new(b"s");
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
        let id = OwnedSubId::new(b"s");
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
            seed_range(&writer, &OwnedSubId::new(b"s"), 2, 2).await;
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
        let id = OwnedSubId::new(b"s");
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
}
