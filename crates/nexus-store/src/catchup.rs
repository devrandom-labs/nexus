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

    /// Open a bounded scan resuming at `pos`.
    fn read_after(
        &self,
        pos: Self::Position,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;

    /// Arm a wait for new events. The returned future is `'static` and
    /// lost-wakeup-safe per the [`WakeRegistration`] contract.
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;

    /// The position of an envelope this target yields.
    fn position_of(env: &PersistedEnvelope) -> Self::Position;

    /// The position the first scan starts at, given a resume point.
    ///
    /// `None` = from the beginning; `Some(p)` = the position *after* `p`
    /// (strict greater-than / keyset resume).
    fn start(from: Option<Self::Position>) -> Self::Position;
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

    fn read_after(
        &self,
        pos: Version,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_stream(&self.id, pos)
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }

    fn position_of(env: &PersistedEnvelope) -> Version {
        env.version()
    }

    fn start(from: Option<Version>) -> Version {
        // Start the scan at the event AFTER `from` (strict greater-than /
        // keyset resume); `None` = from the first event. On `Version::MAX`
        // overflow, stay at MAX (the scan is simply empty — overflow
        // correctness is the loop's concern, kept total here).
        from.map_or(Version::INITIAL, |v| v.next().unwrap_or(v))
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

    fn read_after(
        &self,
        pos: GlobalSeq,
    ) -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_all(pos)
    }

    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        self.reg.arm()
    }

    fn position_of(env: &PersistedEnvelope) -> GlobalSeq {
        env.global_seq()
    }

    fn start(from: Option<GlobalSeq>) -> GlobalSeq {
        // `read_all`'s `from` is INCLUSIVE, so resume at the position AFTER
        // `from`. `None` = from the first global position. On overflow, stay
        // at MAX (the scan is simply empty).
        from.map_or(GlobalSeq::INITIAL, |g| g.next().unwrap_or(g))
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
        let scan = catchup.read_after(Version::INITIAL).await.unwrap();
        let versions: Vec<u64> = scan.map(|r| r.unwrap().version().as_u64()).collect().await;

        assert_eq!(
            versions,
            vec![1, 2, 3],
            "per-stream catchup must yield versions 1,2,3 in order"
        );
    }
}
