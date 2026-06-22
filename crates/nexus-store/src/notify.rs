//! Per-stream wake registry with a deterministic (drop-guard) lifecycle.
//!
//! # What this is
//!
//! The *ephemeral, in-process* half of the subscription wake-up path. It holds
//! one [`Notify`] per stream that currently has at least one live subscriber.
//! After an adapter durably commits event(s) to stream `X`, it calls
//! [`StreamNotifiers::wake`] with `X`; the stream's notifier wakes every parked
//! subscriber at once via its intrusive waiter list. A stream with no current
//! subscribers costs one map miss and nothing else.
//!
//! This replaces the single store-wide `Notify` (which woke *every* subscriber
//! on *every* commit — an O(subscribers) thundering herd) with O(1) wake-by-
//! stream routing.
//!
//! # What this is NOT
//!
//! This is wake-*routing* only. It does not track subscriber identity, cursor
//! position, or anything durable. A [`Notify`] is a handle to a parked task in
//! *this* process — it cannot be persisted and has no meaning across a restart.
//! Durable, resumable subscriptions (e.g. an actor that passivates and later
//! resumes from its last position) are a separate, higher-layer concern that
//! persists a cursor; this registry is the in-memory wake handle that such a
//! subscription creates while it is active.
//!
//! # Lifecycle — drop-guard
//!
//! An entry exists *iff* at least one [`SubscriptionGuard`] for that stream is
//! alive. [`subscribe`](StreamNotifiers::subscribe) creates-or-reuses the entry
//! and increments a subscriber count; dropping the returned guard decrements it
//! and removes the entry when it reaches zero. The map therefore holds an entry
//! per *currently-active* stream, not per stream ever seen — bounded memory,
//! truthful [`active_streams`](StreamNotifiers::active_streams), and no sweep
//! task (cleanup is synchronous in `Drop`, so no async runtime is required).
//!
//! Drop-guard is chosen over a `Weak<Notify>` + lazy-cleanup scheme because the
//! intended workload (per-entity streams under an actor model that passivates
//! and reactivates constantly) wants the entry's lifetime to equal "a task is
//! parked here", reaped the instant the last subscriber leaves. Lazy cleanup
//! would accumulate one dead entry per passivated stream until a sweep ran, and
//! a sweep needs a timer/runtime this layer deliberately avoids.
//!
//! # Ordering contract
//!
//! Callers MUST register the wait *before* performing the read that could miss
//! the event, and producers MUST call [`wake`](StreamNotifiers::wake) *after*
//! the commit is durable. Together these close the lost-wakeup race: a
//! subscriber either is registered when the producer wakes, or performs its
//! read after the data is already visible.
//!
//! Registration is subtle. [`wake`](StreamNotifiers::wake) calls
//! [`Notify::notify_waiters`], which stores **no** permit and wakes only
//! waiters *already in the list*. A `tokio` [`Notified`] future joins that list
//! only when first **polled**, not when created — so merely calling
//! `notifier().notified()` does not register it. Callers must `pin!` the
//! `Notified` and call [`Notified::enable`] *before* the read:
//!
//! ```ignore
//! let notified = guard.notifier().notified();
//! tokio::pin!(notified);
//! let _ = notified.as_mut().enable(); // join the waiter list NOW
//! // ... read; if still empty ...
//! notified.await;                     // a wake during the read is not lost
//! ```
//!
//! [`Notified`]: tokio::sync::futures::Notified
//! [`Notified::enable`]: tokio::sync::futures::Notified::enable
//! [`Notify::notify_waiters`]: tokio::sync::Notify::notify_waiters

use std::collections::HashMap;
use std::sync::Arc;

use foldhash::fast::RandomState;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::sync::watch;

/// Errors produced by [`StreamNotifiers`].
#[derive(Debug, Error)]
pub enum NotifyError {
    /// The live-subscriber count for a single stream would exceed `usize::MAX`.
    ///
    /// Unreachable in practice — the count is bounded by the number of live
    /// [`SubscriptionGuard`]s, which is bounded by available memory. Modelled
    /// as a returned error rather than a panic to honour the project's
    /// arithmetic-safety rule (no bare arithmetic, no silent saturation).
    #[error("live-subscriber count overflow for a single stream")]
    SubscriberOverflow,
}

/// In-memory, per-stream wake registry. Cheap to share via `Arc`.
#[derive(Debug)]
pub struct StreamNotifiers {
    // A single `Mutex` over a `foldhash`-hashed map. Two facts drove this over a
    // sharded/lock-free map (`dashmap`, `papaya`) on the IoT/mobile target:
    //
    //  - Contention is not the bottleneck at this scale. Each critical section
    //    is a lookup + one `Arc::clone`; lock occupancy ≈ ops × section-time, so
    //    even ~10k wakes/sec on a low-core device sits near ~1% occupancy. The
    //    hasher, by contrast, runs on *every* op, so swapping SipHash → foldhash
    //    is the unconditional win. (hashbrown made foldhash its default in 0.15:
    //    rust-lang/hashbrown#563.)
    //  - `papaya`/`scc`/`flurry` use epoch/RCU reclamation, which defers freeing
    //    a removed node past its logical removal — memory-scarce-hostile and in
    //    tension with this module's deterministic reap-at-zero. A `Mutex` frees
    //    the entry the instant `release` removes it.
    //
    // If profiling on real high-core hardware ever shows this global lock
    // contended, `dashmap` is a drop-in with the same API and the same foldhash
    // hasher; revisit then, not before.
    map: Mutex<HashMap<Box<[u8]>, Entry, RandomState>>,
    /// Store-wide notifier for `$all` subscribers. Always present (no
    /// drop-guard / refcount): every commit wakes it, and every `$all`
    /// subscriber genuinely wants every event, so there is no thundering
    /// herd to avoid here — unlike the per-stream `map`.
    all: Arc<Notify>,
    /// Store-wide `$all` generation counter, bumped on every [`wake_all`]. A
    /// `watch` channel alongside [`all`](Self::all): the `Notify` rouses parked
    /// tasks, this exposes a monotone (wrapping) generation a cursor can read
    /// and compare to detect a missed wake without parking.
    all_gen_tx: watch::Sender<u64>,
}

impl Default for StreamNotifiers {
    fn default() -> Self {
        Self {
            map: Mutex::default(),
            all: Arc::new(Notify::new()),
            all_gen_tx: watch::Sender::new(0),
        }
    }
}

#[derive(Debug)]
struct Entry {
    notify: Arc<Notify>,
    /// Per-stream generation counter, bumped on every [`wake`] for this stream.
    /// The `watch` generation counterpart to the [`notify`](Self::notify)
    /// field: the `Notify` rouses parked tasks, this exposes a monotone
    /// (wrapping) generation a cursor can read and compare to detect a missed
    /// wake without parking.
    gen_tx: watch::Sender<u64>,
    /// Number of live [`SubscriptionGuard`]s for this stream.
    subscribers: usize,
}

impl StreamNotifiers {
    /// Create an empty registry behind an `Arc`.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register interest in `stream`, returning a guard that keeps the stream's
    /// notifier alive for as long as it is held.
    ///
    /// Park on [`SubscriptionGuard::notifier`]; drop the guard to unsubscribe.
    ///
    /// # Errors
    ///
    /// [`NotifyError::SubscriberOverflow`] if the live-subscriber count for the
    /// stream would overflow `usize` (unreachable in practice).
    pub fn subscribe(self: &Arc<Self>, stream: &[u8]) -> Result<SubscriptionGuard, NotifyError> {
        let key: Box<[u8]> = Box::from(stream);
        let mut map = self.map.lock();
        let entry = map.entry(key.clone()).or_insert_with(|| Entry {
            notify: Arc::new(Notify::new()),
            gen_tx: watch::Sender::new(0),
            subscribers: 0,
        });
        entry.subscribers = entry
            .subscribers
            .checked_add(1)
            .ok_or(NotifyError::SubscriberOverflow)?;
        let notify = Arc::clone(&entry.notify);
        drop(map);
        Ok(SubscriptionGuard {
            registry: Arc::clone(self),
            key,
            notify,
        })
    }

    /// Wake every task currently parked on `stream`. No-op when the stream has
    /// no live subscribers.
    ///
    /// MUST be called *after* the corresponding event(s) are durably committed,
    /// so that a woken subscriber re-reads already-visible data.
    pub fn wake(&self, stream: &[u8]) {
        // Clone the `Arc` out under the lock, then release it before waking, so
        // woken subscribers don't immediately contend on the map lock they have
        // no need for.
        let maybe_notify = {
            let map = self.map.lock();
            map.get(stream).map(|entry| {
                entry.gen_tx.send_modify(|g| *g = g.wrapping_add(1));
                Arc::clone(&entry.notify)
            })
        };
        if let Some(notify) = maybe_notify {
            notify.notify_waiters();
        }
    }

    /// Wake every task parked on the store-wide `$all` notifier.
    ///
    /// MUST be called *after* the corresponding event(s) are durably
    /// committed, so a woken `$all` subscriber re-reads visible data. A no-op
    /// when no `$all` subscriber is parked.
    pub fn wake_all(&self) {
        self.all_gen_tx.send_modify(|g| *g = g.wrapping_add(1));
        self.all.notify_waiters();
    }

    /// The store-wide `$all` notifier. An all-streams subscription cursor
    /// clones this and parks on it; obey the same enable-before-read ordering
    /// as the per-stream path (see the module-level ordering contract).
    #[must_use]
    pub const fn all_notifier(&self) -> &Arc<Notify> {
        &self.all
    }

    /// Current per-stream wake generation for `stream`, or `0` when the stream
    /// has no live entry. Bumped by every [`wake`](Self::wake); a cursor reads
    /// it to detect a wake that arrived between two reads. Wrapping, so compare
    /// for inequality, not ordering.
    ///
    /// The `0`-for-absent-stream return is unambiguous in practice: a
    /// subscription cursor reads this only while holding its own live
    /// [`SubscriptionGuard`], so its entry is guaranteed present and a genuine
    /// generation of `0` (the entry's initial value) cannot be confused with
    /// the absent-stream `0`.
    #[must_use]
    pub fn generation(&self, stream: &[u8]) -> u64 {
        self.map
            .lock()
            .get(stream)
            .map_or(0, |e| *e.gen_tx.borrow())
    }

    /// Current store-wide `$all` wake generation. Bumped by every
    /// [`wake_all`](Self::wake_all). Wrapping, so compare for inequality, not
    /// ordering.
    #[must_use]
    pub fn all_generation(&self) -> u64 {
        *self.all_gen_tx.borrow()
    }

    /// Number of streams with at least one live subscriber. Diagnostics only.
    #[must_use]
    pub fn active_streams(&self) -> usize {
        self.map.lock().len()
    }

    /// Drop-guard back-channel: decrement a stream's subscriber count and remove
    /// the entry when it reaches zero. Atomic under the map lock, so it cannot
    /// race a concurrent `subscribe` for the same key into a lost wakeup.
    fn release(&self, key: &[u8]) {
        let mut map = self.map.lock();
        let Some(entry) = map.get_mut(key) else {
            // No entry → a guard outlived its entry. Impossible by construction;
            // nothing to do.
            return;
        };
        match entry.subscribers.checked_sub(1) {
            // Last subscriber (or an impossible underflow) → reap the entry.
            Some(0) | None => {
                map.remove(key);
            }
            Some(remaining) => entry.subscribers = remaining,
        }
    }
}

/// RAII handle keeping a stream's notifier registered in a [`StreamNotifiers`].
///
/// While alive, the stream's [`Notify`] stays in the map and is wakeable by the
/// producer. On drop, the stream's subscriber count is decremented and the entry
/// removed once it reaches zero. Carry this inside the subscription cursor's
/// state so it drops exactly when the cursor is dropped (e.g. on passivation).
#[derive(Debug)]
pub struct SubscriptionGuard {
    registry: Arc<StreamNotifiers>,
    key: Box<[u8]>,
    notify: Arc<Notify>,
}

impl SubscriptionGuard {
    /// The shared [`Notify`] for this stream.
    ///
    /// Obtain the wait future with `notifier().notified()`, then `pin!` it and
    /// call [`Notified::enable`](tokio::sync::futures::Notified::enable) to join
    /// the waiter list *before* the refill read; await it only *after* the read
    /// comes back empty. Enabling before the read is what makes a concurrent
    /// commit's wake un-missable — see the module-level ordering contract.
    #[must_use]
    pub const fn notifier(&self) -> &Arc<Notify> {
        &self.notify
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::expect_used, reason = "test code")]
#[allow(
    clippy::similar_names,
    reason = "notifier/notified are domain-accurate names"
)]
#[allow(
    clippy::shadow_reuse,
    reason = "Arc::clone rebinds are idiomatic for task captures"
)]
mod tests {
    use super::{Arc, StreamNotifiers};
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tokio::time::timeout;

    /// Generous upper bound on "a wake must arrive". Far longer than any real
    /// scheduling delay, so a timeout here means a genuinely lost wakeup.
    const MUST_WAKE: Duration = Duration::from_secs(5);
    /// Short bound for asserting the negative: "this waiter must NOT be woken".
    const MUST_NOT_WAKE: Duration = Duration::from_millis(150);

    /// Spawn a task that enables its wait *before* signalling readiness, then
    /// parks. Returns the join handle and a barrier the caller waits on to know
    /// the waiter is registered (so a subsequent `wake` cannot be lost).
    fn park_enabled(
        reg: &Arc<StreamNotifiers>,
        key: &'static [u8],
    ) -> (tokio::task::JoinHandle<()>, Arc<Barrier>) {
        let guard = reg.subscribe(key).unwrap();
        let notifier = Arc::clone(guard.notifier());
        let ready = Arc::new(Barrier::new(2));
        let ready_sub = Arc::clone(&ready);
        let handle = tokio::spawn(async move {
            // Hold the guard for the task's whole life so the entry stays live.
            let _guard = guard;
            let notified = notifier.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable(); // join waiter list NOW
            ready_sub.wait().await; // tell the caller we are registered
            notified.await;
        });
        (handle, ready)
    }

    // ───────────────────────── Category 1: sequence / protocol ─────────────────────────

    /// subscribe → enable → wake rouses the parked waiter, in that order.
    #[tokio::test]
    async fn subscribe_then_wake_rouses_waiter() {
        let reg = StreamNotifiers::new();
        let (sub, ready) = park_enabled(&reg, b"s1");
        assert_eq!(reg.active_streams(), 1);

        ready.wait().await; // waiter is enabled and about to park
        reg.wake(b"s1");
        timeout(MUST_WAKE, sub)
            .await
            .expect("waiter must wake after wake()")
            .expect("subscriber task panicked");
    }

    /// A full subscribe → wake → drop sequence leaves the registry empty.
    #[tokio::test]
    async fn subscribe_wake_drop_sequence_leaves_no_entry() {
        let reg = StreamNotifiers::new();
        let guard = reg.subscribe(b"s1").unwrap();
        reg.wake(b"s1"); // live subscriber, none parked: a harmless no-op
        assert_eq!(reg.active_streams(), 1);
        drop(guard);
        assert_eq!(reg.active_streams(), 0);
    }

    // ───────────────────────── Category 2: lifecycle ─────────────────────────

    /// Two subscribers on one stream share ONE entry and ONE notifier; the
    /// entry is reaped only when the last guard drops.
    #[tokio::test]
    async fn refcount_reap_at_zero() {
        let reg = StreamNotifiers::new();
        let g1 = reg.subscribe(b"k").unwrap();
        let g2 = reg.subscribe(b"k").unwrap();
        // One stream entry, not two.
        assert_eq!(reg.active_streams(), 1);
        // Both guards observe the SAME underlying Notify, so a single wake
        // rouses every subscriber of the stream.
        assert!(Arc::ptr_eq(g1.notifier(), g2.notifier()));

        drop(g1);
        assert_eq!(reg.active_streams(), 1); // one subscriber remains
        drop(g2);
        assert_eq!(reg.active_streams(), 0); // reaped at zero
    }

    /// After an entry is reaped, re-subscribing builds a FRESH notifier — the
    /// old one was not silently kept alive.
    #[tokio::test]
    async fn resubscribe_after_reap_is_fresh() {
        let reg = StreamNotifiers::new();
        let first = reg.subscribe(b"k").unwrap();
        let first_notifier = Arc::clone(first.notifier());
        drop(first);
        assert_eq!(reg.active_streams(), 0);

        let second = reg.subscribe(b"k").unwrap();
        assert_eq!(reg.active_streams(), 1);
        assert!(
            !Arc::ptr_eq(&first_notifier, second.notifier()),
            "reaped entry must not be reused: re-subscribe must allocate a new Notify"
        );
    }

    /// Distinct streams get independent entries and notifiers.
    #[tokio::test]
    async fn distinct_streams_are_independent() {
        let reg = StreamNotifiers::new();
        let a = reg.subscribe(b"a").unwrap();
        let b = reg.subscribe(b"b").unwrap();
        assert_eq!(reg.active_streams(), 2);
        assert!(!Arc::ptr_eq(a.notifier(), b.notifier()));
        drop(a);
        assert_eq!(reg.active_streams(), 1);
        drop(b);
        assert_eq!(reg.active_streams(), 0);
    }

    // ───────────────────────── Category 3: defensive boundary ─────────────────────────

    /// wake on a stream with no subscribers is a no-op and never panics.
    #[tokio::test]
    async fn wake_with_no_subscribers_is_noop() {
        let reg = StreamNotifiers::new();
        reg.wake(b"never-subscribed");
        assert_eq!(reg.active_streams(), 0);
    }

    /// The empty byte slice is a valid stream key end-to-end (subscribe →
    /// wake → reap).
    #[tokio::test]
    async fn empty_key_is_valid() {
        let reg = StreamNotifiers::new();
        let (sub, ready) = park_enabled(&reg, b"");
        assert_eq!(reg.active_streams(), 1);
        ready.wait().await;
        reg.wake(b"");
        timeout(MUST_WAKE, sub)
            .await
            .expect("empty-key waiter must wake")
            .unwrap();
        assert_eq!(reg.active_streams(), 0); // guard dropped inside the task
    }

    /// A waiter on the store-wide $all notifier wakes on `wake_all()`, and is
    /// NOT roused by a per-stream `wake()`.
    #[tokio::test]
    async fn wake_all_rouses_all_waiter_not_per_stream_wake() {
        let reg = StreamNotifiers::new();
        let notifier = Arc::clone(reg.all_notifier());
        let start = Arc::new(Barrier::new(2));
        let start_sub = Arc::clone(&start);
        let mut sub = tokio::spawn(async move {
            let notified = notifier.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable(); // join waiter list BEFORE signalling
            start_sub.wait().await;
            notified.await;
        });
        start.wait().await; // waiter is enabled and about to park

        // A per-stream wake must NOT rouse the $all waiter.
        reg.wake(b"some-stream");
        assert!(
            timeout(MUST_NOT_WAKE, &mut sub).await.is_err(),
            "a per-stream wake must not rouse the $all waiter"
        );

        // wake_all must rouse it.
        reg.wake_all();
        timeout(MUST_WAKE, sub)
            .await
            .expect("wake_all must rouse the $all waiter")
            .unwrap();
    }

    /// A wake for a DIFFERENT stream must not rouse this stream's waiter; a wake
    /// for the correct stream then does.
    #[tokio::test]
    async fn wake_is_isolated_per_stream() {
        let reg = StreamNotifiers::new();
        let (mut sub, ready) = park_enabled(&reg, b"A");
        ready.wait().await;

        // Wrong stream: must NOT wake the waiter on "A".
        reg.wake(b"B");
        assert!(
            timeout(MUST_NOT_WAKE, &mut sub).await.is_err(),
            "a wake for stream B must not rouse a waiter on stream A"
        );

        // Right stream: now it wakes.
        reg.wake(b"A");
        timeout(MUST_WAKE, sub)
            .await
            .expect("waiter must wake on its own stream")
            .unwrap();
    }

    // ───────────────────────── Category 4: linearizability / isolation ─────────────────────────

    /// Concurrent subscribe / drop / wake churn on ONE key must never leave an
    /// orphaned entry: once every guard is dropped, the registry is empty. This
    /// fails if reap-at-zero races a concurrent subscribe (lost decrement or a
    /// dangling entry).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_churn_leaves_no_orphan() {
        let reg = StreamNotifiers::new();
        let key: &[u8] = b"race";
        let workers = 16usize;
        let iterations = 200usize;
        let barrier = Arc::new(Barrier::new(workers + 1));

        let mut handles = Vec::with_capacity(workers + 1);
        for _ in 0..workers {
            let reg = Arc::clone(&reg);
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                barrier.wait().await; // all workers start together
                for _ in 0..iterations {
                    let guard = reg.subscribe(key).unwrap();
                    reg.wake(key);
                    drop(guard);
                }
            }));
        }
        // A concurrent waker hammering the same key throughout the churn.
        {
            let reg = Arc::clone(&reg);
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                for _ in 0..(workers * iterations) {
                    reg.wake(key);
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(
            reg.active_streams(),
            0,
            "concurrent subscribe/drop/wake churn orphaned a registry entry"
        );
    }

    /// Under real concurrency, a waiter enabled before a concurrent wake is
    /// never lost. The subscriber `enable()`s before the start barrier; the
    /// producer wakes after it — so the wake always finds a registered waiter.
    /// Repeated to shake out scheduling races.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_wake_is_not_lost() {
        for _ in 0..50 {
            let reg = StreamNotifiers::new();
            let guard = reg.subscribe(b"k").unwrap();
            let notifier = Arc::clone(guard.notifier());
            let start = Arc::new(Barrier::new(2));

            let start_sub = Arc::clone(&start);
            let sub = tokio::spawn(async move {
                let notified = notifier.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable(); // registered BEFORE the race
                start_sub.wait().await;
                notified.await;
            });

            let start_prod = Arc::clone(&start);
            let reg_prod = Arc::clone(&reg);
            let prod = tokio::spawn(async move {
                start_prod.wait().await;
                reg_prod.wake(b"k");
            });

            timeout(MUST_WAKE, sub)
                .await
                .expect("an enabled waiter must not lose a concurrent wake")
                .unwrap();
            prod.await.unwrap();
            drop(guard);
            assert_eq!(reg.active_streams(), 0);
        }
    }

    #[tokio::test]
    async fn wake_increments_stream_generation() {
        let reg = StreamNotifiers::new();
        let _guard = reg.subscribe(b"s1").unwrap();
        let before = reg.generation(b"s1");
        reg.wake(b"s1");
        let after = reg.generation(b"s1");
        assert_eq!(
            after,
            before + 1,
            "wake must bump the stream generation by 1"
        );
    }

    #[tokio::test]
    async fn wake_all_increments_all_generation() {
        let reg = StreamNotifiers::new();
        let before = reg.all_generation();
        reg.wake_all();
        assert_eq!(reg.all_generation(), before + 1);
    }

    /// Waking one stream bumps only its generation, not another's.
    #[tokio::test]
    async fn generations_are_independent_per_stream() {
        let reg = StreamNotifiers::new();
        let _a = reg.subscribe(b"a").unwrap();
        let _b = reg.subscribe(b"b").unwrap();
        let a_before = reg.generation(b"a");
        let b_before = reg.generation(b"b");
        reg.wake(b"a");
        assert_eq!(
            reg.generation(b"a"),
            a_before + 1,
            "wake(a) must bump a's generation"
        );
        assert_eq!(
            reg.generation(b"b"),
            b_before,
            "wake(a) must not touch b's generation"
        );
    }

    /// A never-subscribed stream reports generation 0.
    #[tokio::test]
    async fn generation_of_unknown_stream_is_zero() {
        let reg = StreamNotifiers::new();
        assert_eq!(reg.generation(b"never"), 0);
    }
}
