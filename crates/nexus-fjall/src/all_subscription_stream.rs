use std::sync::Arc;

use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;
use nexus_store::store::RawEventStore;

use crate::all_stream::FjallAllStream;
use crate::error::FjallError;
use crate::store::FjallStore;

// ═══════════════════════════════════════════════════════════════════════════
// FjallAllSubscriptionStream — Arc-owning $all subscription cursor
// ═══════════════════════════════════════════════════════════════════════════

/// `futures::Stream` of all-streams events in [`GlobalSeq`] order.
///
/// The `$all` dual of [`FjallSubscriptionStream`]: resumes on
/// [`GlobalSeq`] instead of [`Version`], scans `events_global` in
/// ascending order, and parks on the store-wide `all_notifier()` (not a
/// per-stream guard) when caught up.
///
/// **Never returns `None`** — when caught up it waits for new events
/// rather than terminating. Uses the same enable-before-refill lost-wakeup
/// discipline as the per-stream cursor.
///
/// [`FjallSubscriptionStream`]: crate::subscription_stream::FjallSubscriptionStream
/// [`Version`]: nexus::Version
pub struct FjallAllSubscriptionStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, FjallError>> + Send>,
    >,
}

impl FjallAllSubscriptionStream {
    pub(crate) fn new(store: Arc<FjallStore>, inner: FjallAllStream, start: u64) -> Self {
        let state = AllSubState {
            store,
            inner,
            next_global_seq: start,
        };
        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                // Yield from the current inner batch if available.
                if let Some(item) = s.inner.poll_one() {
                    match item {
                        Ok(env) => {
                            // Advance past this event; checked_add to prevent overflow.
                            match env.global_seq().as_u64().checked_add(1) {
                                Some(next) => s.next_global_seq = next,
                                None => {
                                    // global_seq is at u64::MAX — cannot advance.
                                    return Some((Err(FjallError::GlobalSeqOverflow), s));
                                }
                            }
                            return Some((Ok(env), s));
                        }
                        Err(e) => return Some((Err(e), s)),
                    }
                }

                // Buffer empty. Register the waiter BEFORE refilling to close
                // the lost-wakeup race. `wake_all` uses `notify_waiters` (no
                // stored permit) and a `Notified` only joins the waiter list
                // when polled, so `enable()` must register it before the refill
                // read — otherwise a `wake_all` landing during `refill` is lost.
                let notify = Arc::clone(s.store.notifiers.all_notifier());
                let notified = notify.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable();

                if let Err(e) = s.refill().await {
                    return Some((Err(e), s));
                }
                if s.inner.is_empty() {
                    notified.await;
                }
            }
        });
        Self {
            inner: Box::pin(unfolded),
        }
    }
}

impl futures::Stream for FjallAllSubscriptionStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Inner state for the `$all` subscription's `unfold` generator.
struct AllSubState {
    store: Arc<FjallStore>,
    inner: FjallAllStream,
    /// Next `global_seq` to scan from (inclusive). Starts at
    /// `GlobalSeq::INITIAL.as_u64()` or `from + 1`; advanced to
    /// `last_global_seq + 1` after each yield.
    next_global_seq: u64,
}

impl AllSubState {
    async fn refill(&mut self) -> Result<(), FjallError> {
        let from = GlobalSeq::new(self.next_global_seq).unwrap_or(GlobalSeq::INITIAL);
        let fresh = self.store.read_all(from).await?;
        self.inner = fresh;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use crate::store::FjallStore;
    use futures::StreamExt;
    use nexus::Version;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::store::{GlobalSeq, RawEventStore};
    use nexus_store::subscription::RawAllSubscription;

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

    fn temp_store() -> (Arc<FjallStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        (Arc::new(store), dir)
    }

    fn mk_env(version: u64, payload: &[u8]) -> nexus_store::PendingEnvelope {
        pending_envelope(Version::new(version).unwrap())
            .event_type("E")
            .payload(payload.to_vec())
            .unwrap()
            .build()
    }

    #[tokio::test]
    async fn subscribe_all_catches_up_then_sees_live_event() {
        let (store, _dir) = temp_store();
        let a = tid("a");
        let b = tid("b");

        // Pre-seed: a@1 (global_seq=1), b@1 (global_seq=2).
        store.append(&a, None, &[mk_env(1, b"a1")]).await.unwrap();
        store.append(&b, None, &[mk_env(1, b"b1")]).await.unwrap();

        // Open the $all cursor from the beginning.
        let mut sub = FjallStore::subscribe_all(&store, None).await.unwrap();

        // Catch-up: must yield global_seq 1 then global_seq 2.
        let e1 = sub.next().await.unwrap().unwrap();
        assert_eq!(
            e1.global_seq(),
            GlobalSeq::new(1).unwrap(),
            "first event must have global_seq 1"
        );

        let e2 = sub.next().await.unwrap().unwrap();
        assert_eq!(
            e2.global_seq(),
            GlobalSeq::new(2).unwrap(),
            "second event must have global_seq 2"
        );

        // Live wake: spawn a task that appends a@2 (global_seq=3).
        let store2 = Arc::clone(&store);
        tokio::spawn(async move {
            store2
                .append(&tid("a"), Version::new(1), &[mk_env(2, b"a2")])
                .await
                .unwrap();
        });

        // The cursor must wake and yield the live event.
        let e3 = sub.next().await.unwrap().unwrap();
        assert_eq!(
            e3.global_seq(),
            GlobalSeq::new(3).unwrap(),
            "live event must have global_seq 3"
        );
        assert_eq!(e3.payload(), b"a2", "live event payload must be a2");
    }
}
