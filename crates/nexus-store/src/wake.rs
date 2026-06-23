//! Adapter-pluggable wake mechanism for live subscriptions.
//!
//! The generic subscription loop parks on a [`WakeRegistration`] until new
//! events may exist. In-process adapters use
//! [`StreamNotifiers`](crate::notify::StreamNotifiers); distributed adapters
//! (e.g. postgres) implement these traits over `LISTEN`/`NOTIFY`.
//!
//! Only ever used as a generic bound (never `dyn`), so `arm` is an RPITIT
//! future — no associated `Wait` type, no boxing.

use core::future::Future;

/// A live wake source. `register` returns a handle that keeps wake-routing
/// alive for a target (a stream, or `$all` when `stream` is `None`).
pub trait WakeSource: Send + Sync + 'static {
    /// Handle keeping wake-routing alive; dropped when the subscription ends.
    type Registration: WakeRegistration;
    /// Failure to register (e.g. a subscriber-count overflow).
    type Error: core::error::Error + Send + Sync + 'static;

    /// Register interest in `stream` (`None` = `$all`).
    ///
    /// # Errors
    /// Adapter-specific registration failure.
    fn register(&self, stream: Option<&[u8]>) -> Result<Self::Registration, Self::Error>;

    /// Signal that new events for `stream` (and therefore `$all`) are durably
    /// committed. MUST be called by the adapter *after* commit.
    fn wake(&self, stream: &[u8]);
}

/// An arm-able registration. `arm` returns an owned, lost-wakeup-safe future.
pub trait WakeRegistration: Send + 'static {
    /// Arm a wait. CONTRACT: the returned future captures a "seen version" at
    /// the moment `arm` is called; awaiting it resolves once a wake is
    /// delivered *after* that point — a wake between `arm` and the await is
    /// NOT lost. Spurious wakes permitted. The future is `'static` (carries no
    /// borrow of `self`).
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::notify::StreamNotifiers;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tokio::time::timeout;

    const MUST_WAKE: Duration = Duration::from_secs(5);

    /// Ported lost-wakeup test: a registration armed before a concurrent wake
    /// must never miss it. Repeated to shake out scheduling races.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn armed_wait_never_loses_a_concurrent_wake() {
        for _ in 0..50 {
            let reg = StreamNotifiers::new();
            let registration = reg.register(Some(b"k")).unwrap();
            let wait = registration.arm(); // armed BEFORE the race
            let start = Arc::new(Barrier::new(2));

            let start_prod = Arc::clone(&start);
            let reg_prod = Arc::clone(&reg);
            let prod = tokio::spawn(async move {
                start_prod.wait().await;
                reg_prod.wake(b"k");
            });

            start.wait().await;
            timeout(MUST_WAKE, wait)
                .await
                .expect("an armed wait must not lose a concurrent wake");
            prod.await.unwrap();
        }
    }

    /// $all registration wakes on any stream's wake.
    #[tokio::test]
    async fn all_registration_wakes_on_any_stream() {
        let reg = StreamNotifiers::new();
        let registration = reg.register(None).unwrap();
        let wait = registration.arm();
        reg.wake(b"any-stream");
        timeout(MUST_WAKE, wait)
            .await
            .expect("$all must wake on any stream wake");
    }

    /// Drives the wake purely through the `WakeSource` trait surface (a generic
    /// bound, the way the subscription loop uses it): register, arm, then call
    /// the *trait* `wake`. Proves the trait method is not the recursive shadow
    /// of the inherent one (a recursion would hang and time out here) and that
    /// a per-stream wake reaches both a per-stream and an `$all` registration.
    #[tokio::test]
    async fn trait_wake_routes_to_stream_and_all() {
        async fn arm_and_wake<W: WakeSource>(src: &W) {
            let per_stream = src.register(Some(b"k")).unwrap();
            let all = src.register(None).unwrap();
            let wait_stream = per_stream.arm();
            let wait_all = all.arm();
            // Trait-dispatched wake (not the inherent method).
            WakeSource::wake(src, b"k");
            timeout(MUST_WAKE, wait_stream)
                .await
                .expect("trait wake must rouse the per-stream registration");
            timeout(MUST_WAKE, wait_all)
                .await
                .expect("trait wake must rouse the $all registration");
        }

        let reg = StreamNotifiers::new();
        arm_and_wake(reg.as_ref()).await;
    }

    /// Dropping a per-stream `WakeReg` reaps the entry through the guard chain.
    #[tokio::test]
    async fn dropping_registration_reaps_entry() {
        let reg = StreamNotifiers::new();
        let registration = reg.register(Some(b"s")).unwrap();
        assert_eq!(
            reg.active_streams(),
            1,
            "register(Some) must create one entry"
        );
        drop(registration);
        assert_eq!(
            reg.active_streams(),
            0,
            "dropping the registration must reap the entry"
        );
    }
}
