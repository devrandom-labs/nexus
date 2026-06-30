//! The store-side seam fusing a bounded position-keyed scan (`RawEventStore`)
//! with a wait (`WakeRegistration`) for ONE subscription target. Two
//! compile-time impls (per-stream, `$all`) let the single live loop in the
//! subscription cursor monomorphize into two branch-free state machines — no
//! `dyn`, no boxing.
//!
//! The seam is consumed by the single generic live loop in
//! [`subscription_cursor`](crate::subscription_cursor), which the user-facing
//! [`Subscription`](crate::Subscription) assembles per call site.

use core::future::Future;
use std::sync::Arc;

use futures::StreamExt;
use nexus::Version;

use crate::envelope::PersistedEnvelope;
use crate::store::RawEventStore;
use crate::stream_id::StreamKey;
use crate::wake::{WakeRegistration, WakeSource};

/// One subscription target's catchup behaviour: a bounded position-keyed scan
/// fused with a wait. The single live loop is written over this trait so the
/// per-stream and `$all` cases monomorphize into two branch-free machines.
///
/// The scan yields **position-tagged** items `(Position, PersistedEnvelope)` —
/// the loop threads the resume position from the tag, never from the
/// (position-free) envelope. Resume is **strictly after** the given position
/// (`Ord`-based, no successor function): [`read_after`](Self::read_after) is the
/// single resume transition.
pub trait Catchup: Send {
    /// The position key the scan resumes from (`Version` for a stream, the
    /// adapter's [`AllPosition`](crate::AllPosition) for `$all`).
    type Position: Copy + Send;
    /// The bounded scan this target opens — a `futures::Stream` of
    /// position-tagged envelopes.
    type Scan: futures::Stream<Item = Result<(Self::Position, PersistedEnvelope), Self::Error>>
        + Send;
    /// The scan/wait error type.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Open a bounded scan of events **strictly after** `from` (`None` = from
    /// the beginning), each item tagged with its position.
    ///
    /// This is the single resume transition. Per-stream maps it onto its
    /// inclusive `Version` backend (`read_stream(from.next())`); `$all` delegates
    /// to the adapter's exclusive [`read_all`](RawEventStore::read_all).
    fn read_after(
        &self,
        from: Option<Self::Position>,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;

    /// Arm a wait for new events. The returned future is `'static` and
    /// lost-wakeup-safe per the [`WakeRegistration`] contract.
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;
}

/// Tag a per-stream read item with its [`Version`] so the position-threading
/// live loop treats the per-stream and `$all` scans uniformly. A free `fn`
/// (not a closure) so it names a `fn`-pointer in [`StreamCatchup::Scan`].
fn tag_version<E>(item: Result<PersistedEnvelope, E>) -> Result<(Version, PersistedEnvelope), E> {
    item.map(|env| (env.version(), env))
}

/// Per-stream catchup: scans one stream by [`Version`], waits on that stream.
///
/// The stream key is held as an owned [`StreamKey`] — it satisfies the
/// `'static` bound the reopened scans need across refills, and it is exactly
/// the type [`RawEventStore::read_stream`] consumes, so no per-refill
/// conversion is required.
pub struct StreamCatchup<S: RawEventStore + WakeSource> {
    store: Arc<S>,
    id: StreamKey,
    reg: <S as WakeSource>::Registration,
}

impl<S: RawEventStore + WakeSource> StreamCatchup<S> {
    /// Register interest in the stream, then build the per-stream catchup.
    ///
    /// # Errors
    /// Adapter-specific registration failure (e.g. subscriber-count overflow).
    pub fn new(store: Arc<S>, id_bytes: &[u8]) -> Result<Self, <S as WakeSource>::Error> {
        let reg = store.register(Some(id_bytes))?;
        Ok(Self {
            store,
            id: StreamKey::from_slice(id_bytes),
            reg,
        })
    }
}

/// The `fn`-pointer that [`StreamCatchup::Scan`]'s `Map` applies — names
/// [`tag_version`] concretely so the associated type stays box-free.
type TagFn<S> = fn(
    Result<PersistedEnvelope, <S as RawEventStore>::Error>,
) -> Result<(Version, PersistedEnvelope), <S as RawEventStore>::Error>;

impl<S: RawEventStore + WakeSource> Catchup for StreamCatchup<S> {
    type Position = Version;
    // Either the mapped (version-tagged) per-stream scan, or — at the
    // `Version::MAX` ceiling, where nothing is strictly after `from` — an empty
    // scan. No box: both arms are concrete `futures` stream types.
    type Scan = futures::future::Either<
        futures::stream::Map<<S as RawEventStore>::Stream, TagFn<S>>,
        futures::stream::Empty<Result<(Version, PersistedEnvelope), <S as RawEventStore>::Error>>,
    >;
    type Error = <S as RawEventStore>::Error;

    fn read_after(
        &self,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        // `read_stream` is INCLUSIVE; "strictly after `from`" resumes at
        // `from.next()`. `None` = from the first event. `Some(MAX)` has no
        // successor → nothing is strictly after it → an empty scan, never a
        // re-read of the ceiling event (the old `next_pos` overflow→park).
        let resume = from.map_or(Some(Version::INITIAL), Version::next);
        // Own the handles the block needs (both one refcount bump) rather than
        // capturing `&self`: the returned future must be `Send`, and `&self` is
        // not (`StreamCatchup` is not `Sync` — `Registration` is not `Sync`).
        let store = Arc::clone(&self.store);
        let id = self.id.clone();
        async move {
            match resume {
                Some(v) => {
                    let scan = store.read_stream(&id, v).await?;
                    let tag: TagFn<S> = tag_version;
                    Ok(futures::future::Either::Left(scan.map(tag)))
                }
                None => Ok(futures::future::Either::Right(futures::stream::empty())),
            }
        }
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }
}

/// `$all` catchup: scans every stream by the adapter's
/// [`AllPosition`](crate::AllPosition), waits on any stream.
pub struct AllCatchup<S: RawEventStore + WakeSource> {
    store: Arc<S>,
    reg: <S as WakeSource>::Registration,
}

impl<S: RawEventStore + WakeSource> AllCatchup<S> {
    /// Register `$all` interest, then build the all-streams catchup.
    ///
    /// # Errors
    /// Adapter-specific registration failure (e.g. subscriber-count overflow).
    pub fn new(store: Arc<S>) -> Result<Self, <S as WakeSource>::Error> {
        let reg = store.register(None)?;
        Ok(Self { store, reg })
    }
}

impl<S: RawEventStore + WakeSource> Catchup for AllCatchup<S> {
    type Position = <S as RawEventStore>::AllPosition;
    // The adapter's `$all` stream is already position-tagged and exclusive, so
    // `read_after` delegates straight to `read_all` with no mapping.
    type Scan = <S as RawEventStore>::AllStream;
    type Error = <S as RawEventStore>::Error;

    fn read_after(
        &self,
        from: Option<Self::Position>,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_all(from)
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }
}

#[cfg(all(test, feature = "testing"))]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::envelope::pending_envelope;
    use crate::testing::InMemoryStore;
    use futures::StreamExt;

    async fn seed(store: &InMemoryStore, id: &StreamKey, lo: u64, hi: u64) {
        for v in lo..=hi {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(b"e".to_vec())
                .unwrap()
                .build();
            store.append(id, Version::new(v - 1), &[env]).await.unwrap();
        }
    }

    /// `read_after(None)` scans the registered stream from the beginning,
    /// yielding position-tagged items in strict version order — the tag matches
    /// the envelope's own version.
    #[tokio::test]
    async fn stream_catchup_reads_after_none() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed(&store, &id, 1, 3).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let scan = catchup.read_after(None).await.unwrap();
        let tagged: Vec<(u64, u64)> = scan
            .map(|r| {
                let (pos, env) = r.unwrap();
                (pos.as_u64(), env.version().as_u64())
            })
            .collect()
            .await;

        assert_eq!(
            tagged,
            vec![(1, 1), (2, 2), (3, 3)],
            "per-stream catchup must yield (Version tag, env version) 1,2,3 in order"
        );
    }

    /// `read_after(Some(v))` is **exclusive**: it opens strictly after `v`, so
    /// seeding [1,2,3] and resuming after version 2 yields only [3].
    #[tokio::test]
    async fn stream_catchup_read_after_is_exclusive() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed(&store, &id, 1, 3).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let scan = catchup
            .read_after(Some(Version::new(2).unwrap()))
            .await
            .unwrap();
        let versions: Vec<u64> = scan.map(|r| r.unwrap().0.as_u64()).collect().await;

        assert_eq!(
            versions,
            vec![3],
            "read_after(Some(2)) yields strictly after 2"
        );
    }

    /// Reopen seam: after draining [1,2,3], reopening strictly after the
    /// last-delivered position must NOT re-deliver event 3.
    #[tokio::test]
    async fn reopen_does_not_redeliver_last_event() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed(&store, &id, 1, 3).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();

        let first: Vec<(Version, PersistedEnvelope)> = catchup
            .read_after(None)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await;
        let versions: Vec<u64> = first.iter().map(|(p, _)| p.as_u64()).collect();
        assert_eq!(versions, vec![1, 2, 3], "initial drain yields 1,2,3");

        // Reopen strictly after the LAST delivered position.
        let (last_pos, _) = first.last().unwrap();
        let second: Vec<u64> = catchup
            .read_after(Some(*last_pos))
            .await
            .unwrap()
            .map(|r| r.unwrap().0.as_u64())
            .collect()
            .await;
        assert!(
            second.is_empty(),
            "reopen must NOT re-deliver event 3 (got {second:?})"
        );
    }

    /// The `Version::MAX` ceiling: `read_after(Some(MAX))` has no successor, so
    /// the scan is empty — never a re-read, never a panic (the `Either::Right`
    /// empty-scan branch that replaces the old `next_pos` overflow→park).
    #[tokio::test]
    async fn stream_catchup_read_after_max_is_empty() {
        let store = Arc::new(InMemoryStore::new());
        let id = StreamKey::from_slice(b"s");
        seed(&store, &id, 1, 1).await;

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let scan = catchup
            .read_after(Some(Version::new(u64::MAX).unwrap()))
            .await
            .unwrap();
        let count = scan.count().await;
        assert_eq!(count, 0, "nothing is strictly after Version::MAX");
    }

    /// `$all` catchup delegates to the adapter: `read_after(None)` yields
    /// position-tagged events across streams in `$all` order, and
    /// `read_after(Some(p))` is exclusive (strictly after `p`).
    #[tokio::test]
    async fn all_catchup_reads_after_none_then_exclusive() {
        let store = Arc::new(InMemoryStore::new());
        // Interleave so `$all` order spans both streams: a@1, b@1, a@2.
        seed(&store, &StreamKey::from_slice(b"a"), 1, 1).await;
        seed(&store, &StreamKey::from_slice(b"b"), 1, 1).await;
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
        let all: Vec<(crate::testing::InMemoryAllPos, PersistedEnvelope)> = catchup
            .read_after(None)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await;
        let positions: Vec<u64> = all.iter().map(|(p, _)| p.as_u64()).collect();
        assert_eq!(
            positions,
            vec![1, 2, 3],
            "$all catchup must yield every stream's events in position order"
        );

        // Resume strictly after the first position → [2, 3].
        let (first_pos, _) = all.first().unwrap();
        let rest: Vec<u64> = AllCatchup::new(Arc::clone(&store))
            .unwrap()
            .read_after(Some(*first_pos))
            .await
            .unwrap()
            .map(|r| r.unwrap().0.as_u64())
            .collect()
            .await;
        assert_eq!(rest, vec![2, 3], "read_after(Some(first)) is exclusive");
    }
}
