//! The store-side seam fusing a bounded position-keyed scan (`RawEventStore`)
//! with a wait (`WakeRegistration`) for ONE subscription target. Two
//! compile-time impls (per-stream, `$all`) let the single live loop in the
//! subscription cursor monomorphize into two branch-free state machines — no
//! `dyn`, no boxing.
//!
//! This module is additive scaffolding: the seam is defined here but consumed
//! only by the single generic live loop (a follow-up task). Until that loop
//! lands, the trait, both impls, and their accessors have no in-crate caller —
//! hence the module-wide `dead_code` allow.
#![allow(
    dead_code,
    reason = "additive seam consumed by the not-yet-wired generic live loop"
)]

use core::future::Future;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::envelope::PersistedEnvelope;
use crate::store::{GlobalSeq, RawEventStore};
use crate::wake::{WakeRegistration, WakeSource};

/// One subscription target's catchup behaviour: a bounded position-keyed scan
/// fused with a wait. The single live loop is written over this trait so the
/// per-stream and `$all` cases monomorphize into two branch-free machines.
pub trait Catchup: Send {
    /// The position key the scan resumes from (`Version` or [`GlobalSeq`]).
    type Position: Copy + Send;
    /// The bounded scan this target opens — a `futures::Stream` of envelopes.
    type Scan: futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send;
    /// The scan/wait error type.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Open a bounded scan of events from `from` INCLUSIVE.
    fn read_from(
        &self,
        from: Self::Position,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;

    /// Arm a wait for new events. The returned future is `'static` and
    /// lost-wakeup-safe per the [`WakeRegistration`] contract.
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;

    /// The position of an envelope this target yields.
    fn position_of(env: &PersistedEnvelope) -> Self::Position;

    /// The next INCLUSIVE read position given the last-delivered position
    /// (`None` = nothing delivered / subscribe from the beginning).
    ///
    /// Applied uniformly by the live loop for both the initial open and every
    /// reopen: `None` resolves to the first position; `Some(v)` resolves to the
    /// position strictly *after* `v` (`v.next()`), giving strict-after resume
    /// with no duplicate of the last-delivered event. Returns `None` only on
    /// overflow (the last-delivered position was the maximum, so no further
    /// event can exist).
    fn next_pos(resume: Option<Self::Position>) -> Option<Self::Position>;
}

/// Owned id satisfying [`Id`]'s `'static` bound across reopened scans.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct OwnedSubId(Vec<u8>);

impl core::fmt::Display for OwnedSubId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match core::str::from_utf8(&self.0) {
            Ok(s) => f.write_str(s),
            Err(_) => write!(f, "<{} bytes>", self.0.len()),
        }
    }
}

impl AsRef<[u8]> for OwnedSubId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Id for OwnedSubId {
    const BYTE_LEN: usize = 0;
}

impl OwnedSubId {
    pub fn new(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

/// Per-stream catchup: scans one stream by [`Version`], waits on that stream.
pub struct StreamCatchup<S: RawEventStore + WakeSource> {
    store: Arc<S>,
    id: OwnedSubId,
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
            id: OwnedSubId::new(id_bytes),
            reg,
        })
    }
}

impl<S: RawEventStore + WakeSource> Catchup for StreamCatchup<S> {
    type Position = Version;
    type Scan = <S as RawEventStore>::Stream;
    type Error = <S as RawEventStore>::Error;

    fn read_from(
        &self,
        from: Version,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_stream(&self.id, from)
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }

    fn position_of(env: &PersistedEnvelope) -> Version {
        env.version()
    }

    fn next_pos(resume: Option<Version>) -> Option<Version> {
        // `read_from` is INCLUSIVE; resume strictly after the last-delivered
        // version so it is not re-read. `None` = from the first event;
        // `None` out = overflow at `Version::MAX` (no further event possible).
        resume.map_or(Some(Version::INITIAL), Version::next)
    }
}

/// `$all` catchup: scans every stream by [`GlobalSeq`], waits on any stream.
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
    type Position = GlobalSeq;
    type Scan = <S as RawEventStore>::AllStream;
    type Error = <S as RawEventStore>::Error;

    fn read_from(
        &self,
        from: GlobalSeq,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_all(from)
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }

    fn position_of(env: &PersistedEnvelope) -> GlobalSeq {
        env.global_seq()
    }

    fn next_pos(resume: Option<GlobalSeq>) -> Option<GlobalSeq> {
        // `read_all`'s `from` is INCLUSIVE; resume strictly after the
        // last-delivered position so it is not re-read. `None` = from the first
        // global position; `None` out = overflow at the maximum (no further
        // event possible).
        resume.map_or(Some(GlobalSeq::INITIAL), GlobalSeq::next)
    }
}

#[cfg(all(test, feature = "testing"))]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::envelope::pending_envelope;
    use crate::testing::InMemoryStore;
    use futures::StreamExt;

    /// A per-stream catchup scans the stream it registered for and yields the
    /// persisted events in strict version order from the requested position.
    #[tokio::test]
    async fn stream_catchup_reads_from_initial() {
        let store = Arc::new(InMemoryStore::new());
        let id = OwnedSubId::new(b"s");

        for v in 1u64..=3 {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(b"e".to_vec())
                .unwrap()
                .build();
            store
                .append(&id, Version::new(v - 1), &[env])
                .await
                .unwrap();
        }

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();
        let rf = <StreamCatchup<InMemoryStore> as Catchup>::next_pos(None).unwrap();
        let scan = catchup.read_from(rf).await.unwrap();
        let versions: Vec<u64> = scan.map(|r| r.unwrap().version().as_u64()).collect().await;

        assert_eq!(
            versions,
            vec![1, 2, 3],
            "per-stream catchup must yield versions 1,2,3 in order"
        );
    }

    /// `next_pos` is the single resume transition: beginning, strict-after, and
    /// overflow-as-`None` for the per-stream [`Version`] key.
    #[test]
    fn version_next_pos_transition() {
        type C = StreamCatchup<InMemoryStore>;

        assert_eq!(
            <C as Catchup>::next_pos(None),
            Some(Version::INITIAL),
            "None resumes from the first version"
        );

        let v = Version::new(7).unwrap();
        assert_eq!(
            <C as Catchup>::next_pos(Some(v)),
            v.next(),
            "Some(v) resumes strictly after v (v.next())"
        );
        assert_eq!(
            <C as Catchup>::next_pos(Some(v)),
            Version::new(8),
            "Some(7) resolves to Some(8)"
        );

        let max = Version::new(u64::MAX).unwrap();
        assert_eq!(
            <C as Catchup>::next_pos(Some(max)),
            None,
            "overflow at MAX surfaces as None, never pinned to MAX"
        );
    }

    /// `next_pos` is the single resume transition for the `$all` [`GlobalSeq`]
    /// key: beginning, strict-after, and overflow-as-`None`.
    #[test]
    fn global_seq_next_pos_transition() {
        type C = AllCatchup<InMemoryStore>;

        assert_eq!(
            <C as Catchup>::next_pos(None),
            Some(GlobalSeq::INITIAL),
            "None resumes from the first global position"
        );

        let g = GlobalSeq::new(7).unwrap();
        assert_eq!(
            <C as Catchup>::next_pos(Some(g)),
            g.next(),
            "Some(g) resumes strictly after g (g.next())"
        );
        assert_eq!(
            <C as Catchup>::next_pos(Some(g)),
            GlobalSeq::new(8),
            "Some(7) resolves to Some(8)"
        );

        let max = GlobalSeq::new(u64::MAX).unwrap();
        assert_eq!(
            <C as Catchup>::next_pos(Some(max)),
            None,
            "overflow at MAX surfaces as None, never pinned to MAX"
        );
    }

    /// Reopen seam: after draining [1,2,3], the reopen position derived from the
    /// last-delivered event must NOT re-deliver event 3. This is the exact
    /// duplicate the old inclusive-`read_after` + `position_of` loop produced.
    #[tokio::test]
    async fn reopen_does_not_redeliver_last_event() {
        let store = Arc::new(InMemoryStore::new());
        let id = OwnedSubId::new(b"s");

        for v in 1u64..=3 {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(b"e".to_vec())
                .unwrap()
                .build();
            store
                .append(&id, Version::new(v - 1), &[env])
                .await
                .unwrap();
        }

        let catchup = StreamCatchup::new(Arc::clone(&store), b"s").unwrap();

        // Initial open: next_pos(None) -> INITIAL, drain [1,2,3].
        let rf = <StreamCatchup<InMemoryStore> as Catchup>::next_pos(None).unwrap();
        let first: Vec<PersistedEnvelope> = catchup
            .read_from(rf)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await;
        let versions: Vec<u64> = first.iter().map(|e| e.version().as_u64()).collect();
        assert_eq!(versions, vec![1, 2, 3], "initial drain yields 1,2,3");

        // Reopen: derive the next read position from the LAST delivered event.
        let last = first.last().unwrap();
        let reopen =
            <StreamCatchup<InMemoryStore> as Catchup>::next_pos(Some(
                StreamCatchup::<InMemoryStore>::position_of(last),
            ))
            .unwrap();
        assert_eq!(
            reopen,
            Version::new(4).unwrap(),
            "reopen position is 4, not 3"
        );

        let second: Vec<u64> = catchup
            .read_from(reopen)
            .await
            .unwrap()
            .map(|r| r.unwrap().version().as_u64())
            .collect()
            .await;
        assert!(
            second.is_empty(),
            "reopen must NOT re-deliver event 3 (got {second:?})"
        );
    }
}
