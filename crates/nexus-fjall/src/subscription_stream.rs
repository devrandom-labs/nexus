use std::sync::Arc;

use arrayvec::ArrayString;
use nexus::{Id, Version};
use nexus_store::PersistedEnvelope;
use nexus_store::notify::SubscriptionGuard;
use nexus_store::store::RawEventStore;

use crate::error::FjallError;
use crate::store::FjallStore;
use crate::stream::FjallStream;

/// Owned byte-key wrapper to satisfy the [`Id`] trait's `'static` bound
/// when re-reading from the store during subscription refills.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct OwnedStreamId(Vec<u8>);

impl OwnedStreamId {
    /// Create from any [`Id`] by capturing its byte representation.
    pub(crate) fn from_id(id: &impl Id) -> Self {
        Self(id.as_ref().to_vec())
    }
}

impl std::fmt::Display for OwnedStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match std::str::from_utf8(&self.0) {
            Ok(s) => f.write_str(s),
            Err(_) => write!(f, "<{} bytes>", self.0.len()),
        }
    }
}

impl AsRef<[u8]> for OwnedStreamId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Id for OwnedStreamId {
    const BYTE_LEN: usize = 0;
}

// ═══════════════════════════════════════════════════════════════════════════
// FjallSubscriptionStream — Arc-owning subscription cursor
// ═══════════════════════════════════════════════════════════════════════════

/// `futures::Stream` subscription that owns an `Arc<FjallStore>`.
///
/// `'static` (no lifetime parameter). Spawnable across async boundaries.
/// Holds an eagerly-loaded batch of event rows (via [`FjallStream`]).
/// When the inner batch is exhausted, it waits on [`FjallStore`]'s
/// `Notify` and re-reads from the last yielded version. The stream
/// **never returns `None`** — it blocks until new events arrive.
pub struct FjallSubscriptionStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, FjallError>> + Send>,
    >,
}

impl FjallSubscriptionStream {
    #[allow(
        clippy::too_many_arguments,
        reason = "cursor constructor threads six distinct pieces of subscription state: store handle, stream key, diagnostic label, initial batch, resume version, and wake guard"
    )]
    pub(crate) fn new(
        store: Arc<FjallStore>,
        stream_key: OwnedStreamId,
        label: ArrayString<64>,
        inner: FjallStream,
        last_version: Option<Version>,
        guard: SubscriptionGuard,
    ) -> Self {
        let state = SubState {
            store,
            stream_key,
            label,
            inner,
            last_version,
            guard,
        };
        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                // Yield from the current inner batch if available.
                if let Some(item) = s.inner.poll_one() {
                    match item {
                        Ok(env) => {
                            s.last_version = Some(env.version());
                            return Some((Ok(env), s));
                        }
                        Err(e) => return Some((Err(e), s)),
                    }
                }

                // Buffer empty. Register the waiter BEFORE refilling to close
                // the lost-wakeup race. `wake` uses `notify_waiters` (no stored
                // permit) and a `Notified` only joins the waiter list when
                // polled, so `enable()` must register it before the refill read
                // — otherwise a `wake` landing during `refill` is lost. Clone
                // the `Arc` to an owned handle so the `Notified` borrows the
                // local, not `s`, leaving `s` free for the refill and move.
                let notify = Arc::clone(s.guard.notifier());
                let notified = notify.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable();

                let from = match s.next_read_version() {
                    Ok(v) => v,
                    Err(e) => return Some((Err(e), s)),
                };
                if let Err(e) = s.refill(from).await {
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

impl futures::Stream for FjallSubscriptionStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Inner state for the subscription's `unfold` generator.
struct SubState {
    store: Arc<FjallStore>,
    stream_key: OwnedStreamId,
    #[allow(dead_code, reason = "label is used by future error paths")]
    label: ArrayString<64>,
    inner: FjallStream,
    last_version: Option<Version>,
    /// Keeps this stream's wake entry registered for the cursor's whole
    /// life and supplies the per-stream `Notify`. Dropped with the cursor,
    /// which reaps the entry once the last subscriber leaves.
    guard: SubscriptionGuard,
}

impl SubState {
    fn next_read_version(&self) -> Result<Version, FjallError> {
        self.last_version.map_or(Ok(Version::INITIAL), |v| {
            v.next().ok_or(FjallError::VersionOverflow)
        })
    }

    async fn refill(&mut self, from: Version) -> Result<(), FjallError> {
        let fresh = self.store.read_stream(&self.stream_key, from).await?;
        self.inner = fresh;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::store::batch_test_helpers::{seed, store_with_batch, tid};
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::subscription::RawSubscription;

    #[tokio::test]
    async fn subscription_drains_many_batches_then_sees_live_event() {
        // 10× batch_size backlog: batch_size 4, 40 pre-seeded events (10 full refills).
        let (raw_store, _dir) = store_with_batch(4);
        let store = Arc::new(raw_store);
        let id = tid("s");
        seed(&store, &id, 40).await; // 10 bounded catch-up refills (4×10)

        let mut sub = FjallStore::subscribe(&store, &id, None).await.unwrap();
        for expected in 1..=40u64 {
            let got = sub.next().await.unwrap().unwrap();
            assert_eq!(got.version().as_u64(), expected);
        }

        let store2 = Arc::clone(&store);
        let id2 = id.clone();
        tokio::spawn(async move {
            let env = pending_envelope(Version::new(41).unwrap())
                .event_type("E")
                .payload(vec![41u8])
                .unwrap()
                .build();
            store2.append(&id2, Version::new(40), &[env]).await.unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.version().as_u64(), 41);
    }
}
