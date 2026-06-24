# nexus-fjall cursor unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the common catch-up-then-tail subscription machinery from `nexus-fjall` into `nexus-store` as a zero-cost (no-`Box`) generic, leaving `nexus-fjall` holding only fjall-specific code, and fix the rule violations the audit found.

**Architecture:** `nexus-store` gains a `WakeSource`/`WakeRegistration` wake interface, a `Catchup` seam, and one generic live-subscription loop returned as `impl Stream` (RPIT) — no `Box<dyn>`. `StreamNotifiers` moves from `Notify`+`enable()` to a `watch`-generation mechanism so the wait future is owned (`'static`). `nexus-fjall` collapses its two bounded cursors into one private `ScanStrategy`/`ScanCursor` over a single lazy `fjall::Iter`, deletes both subscription-cursor files, decomposes `append`, and tidies its encoding module.

**Tech Stack:** Rust edition 2024 (~1.96 nightly), `fjall 3.1.4`, `tokio::sync::watch`, `futures::Stream`, `futures::stream::unfold`, RPIT/RPITIT (stable 1.75).

**Spec:** `docs/plans/2026-06-22-nexus-fjall-cursor-unification-design.md`

**Branch:** `refactor/fjall-cursor-unification` (already created).

## Conventions for every task

- **Tests:** run with `nix develop -c cargo test -p <crate> --features testing <filter>` (subscription/InMemory tests need the `testing` feature; add `snapshot`/`projection` where a task says so).
- **Commit = the gate.** The pre-commit hook runs `nix flake check` automatically. Do **not** add a manual "run nix flake check" step and do **not** run the full gate by hand first — just `git add` + `git commit`; the hook gates it. (`feedback_dont_prerun_gate`.)
- **New source files must be `git add`-ed before committing** or the flake build fails on the missing module (`project_nix_flake_check_untracked`).
- **Run `nix develop -c cargo fmt --all`** before staging each commit (`feedback_run_rustfmt_after_edits`).
- **Clippy is god mode.** No `#[allow]` in non-test code without an audit-grade reason; fix the lint instead (`feedback_never_ignore_clippy`).
- Each phase ends with the workspace green. Phases 1–3 are additive (old paths stay live); Phase 4 is the atomic switchover; Phase 5 is independent cleanup.

---

## Phase 1 — `nexus-store`: watch-generation wake + `WakeSource` trait (additive)

Goal: give `StreamNotifiers` a `watch`-based generation alongside its existing `Notify`, and add the `WakeSource`/`WakeRegistration` traits with `StreamNotifiers` as the in-process impl. The old `notifier()`/`all_notifier()`/`subscribe()` API stays intact so the existing fjall cursors keep compiling.

**Files:**
- Modify: `crates/nexus-store/src/notify.rs`
- Create: `crates/nexus-store/src/wake.rs`
- Modify: `crates/nexus-store/src/lib.rs` (declare + re-export `wake`)

### Task 1.1: Add a per-entry + `$all` generation counter to `StreamNotifiers`

- [ ] **Step 1: Write the failing test** — append to the `tests` mod in `crates/nexus-store/src/notify.rs`:

```rust
#[tokio::test]
async fn wake_increments_stream_generation() {
    let reg = StreamNotifiers::new();
    let _guard = reg.subscribe(b"s1").unwrap();
    let before = reg.generation(b"s1");
    reg.wake(b"s1");
    let after = reg.generation(b"s1");
    assert_eq!(after, before + 1, "wake must bump the stream generation by 1");
}

#[tokio::test]
async fn wake_all_increments_all_generation() {
    let reg = StreamNotifiers::new();
    let before = reg.all_generation();
    reg.wake_all();
    assert_eq!(reg.all_generation(), before + 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features testing wake_increments_stream_generation`
Expected: FAIL — `no method named generation`.

- [ ] **Step 3: Implement.** In `crates/nexus-store/src/notify.rs`:
  - Add `use tokio::sync::watch;`.
  - Change `Entry` to carry a generation sender beside the existing `notify`:

```rust
#[derive(Debug)]
struct Entry {
    notify: Arc<Notify>,
    gen_tx: watch::Sender<u64>,
    subscribers: usize,
}
```

  - In `subscribe`, build the entry with `gen_tx: watch::Sender::new(0)` in the `or_insert_with`.
  - Add an `all_gen: watch::Sender<u64>` field to `StreamNotifiers`, initialised `watch::Sender::new(0)` in `Default`.
  - In `wake`, after taking the notify clone, also bump the generation under the lock:

```rust
pub fn wake(&self, stream: &[u8]) {
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
```

  - In `wake_all`, bump `all_gen` then `notify_waiters`:

```rust
pub fn wake_all(&self) {
    self.all_gen.send_modify(|g| *g = g.wrapping_add(1));
    self.all.notify_waiters();
}
```

  - Add read accessors used by the tests (diagnostics):

```rust
#[must_use]
pub fn generation(&self, stream: &[u8]) -> u64 {
    self.map.lock().get(stream).map_or(0, |e| *e.gen_tx.borrow())
}

#[must_use]
pub fn all_generation(&self) -> u64 {
    *self.all_gen.borrow()
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features testing notify`
Expected: PASS (all existing `notify` tests still green — old `Notify` path untouched).

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/notify.rs
git commit -m "feat(store): add watch generation counter to StreamNotifiers"
```

### Task 1.2: Add the `WakeSource` / `WakeRegistration` traits + in-process impl

- [ ] **Step 1: Write the failing test** — create `crates/nexus-store/src/wake.rs` with the traits and tests (impl filled in Step 3):

```rust
//! Adapter-pluggable wake mechanism for live subscriptions.
//!
//! The generic subscription loop ([`crate::subscription`]) parks on a
//! [`WakeRegistration`] until new events may exist. In-process adapters use
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

/// An armed-able registration. `arm` returns an owned, lost-wakeup-safe future.
pub trait WakeRegistration: Send + 'static {
    /// Arm a wait. CONTRACT: the returned future captures a "seen version" at
    /// the moment `arm` is called; awaiting it resolves once a wake is
    /// delivered *after* that point — a wake between `arm` and the await is NOT
    /// lost. Spurious wakes permitted. The future is `'static` (carries no
    /// borrow of `self`).
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;
}
```

Then add tests to the same file:

```rust
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
        timeout(MUST_WAKE, wait).await.expect("$all must wake on any stream wake");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features testing wake::`
Expected: FAIL — `register` not found on `StreamNotifiers`.

- [ ] **Step 3: Implement the in-process impl.** Two parts:

  (a) In `crates/nexus-store/src/notify.rs`, give `wake` the `$all` bump too (so a single `WakeSource::wake` covers both per-stream and `$all`), add a `Registration` type, and the `WakeSource`/`WakeRegistration` impls:

```rust
use crate::wake::{WakeRegistration, WakeSource};

// In `wake`, after bumping the stream gen, also bump $all:
//   self.all_gen.send_modify(|g| *g = g.wrapping_add(1));
//   self.all.notify_waiters();
// (a per-stream commit IS an $all event).

/// In-process registration: a watch receiver on the target's generation,
/// plus (for a per-stream target) the drop-guard that reaps the entry.
#[derive(Debug)]
pub struct WakeReg {
    rx: watch::Receiver<u64>,
    _guard: Option<SubscriptionGuard>,
}

impl WakeRegistration for WakeReg {
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = self.rx.clone();
        rx.mark_unchanged(); // seen = current; only future bumps resolve `changed`
        async move {
            let _ = rx.changed().await; // Err only if all senders dropped; treat as wake
        }
    }
}

impl WakeSource for StreamNotifiers {
    type Registration = WakeReg;
    type Error = NotifyError;

    fn register(&self, stream: Option<&[u8]>) -> Result<WakeReg, NotifyError> {
        match stream {
            None => Ok(WakeReg { rx: self.all_gen.subscribe(), _guard: None }),
            Some(s) => {
                let rx = {
                    let mut map = self.map.lock();
                    let entry = map.entry(Box::from(s)).or_insert_with(|| Entry {
                        notify: Arc::new(Notify::new()),
                        gen_tx: watch::Sender::new(0),
                        subscribers: 0,
                    });
                    entry.subscribers = entry
                        .subscribers
                        .checked_add(1)
                        .ok_or(NotifyError::SubscriberOverflow)?;
                    entry.gen_tx.subscribe()
                };
                // SubscriptionGuard reaps the entry on drop (existing mechanism).
                let guard = self.subscribe_guard_only(s); // see note
                Ok(WakeReg { rx, _guard: Some(guard) })
            }
        }
    }

    fn wake(&self, stream: &[u8]) {
        // bump per-stream gen + notify (existing body), then bump $all gen + notify.
        Self::wake(self, stream);   // reuse the inherent wake (per-stream)
        self.wake_all();            // an append is also an $all event
    }
}
```

  > NOTE on the inherent-vs-trait `wake` name clash: rename the existing inherent `wake`/`wake_all` to keep them, and have the trait `WakeSource::wake` call them. Concretely: keep inherent `pub fn wake`/`pub fn wake_all` as-is, and in the trait impl write the body inline (bump stream gen+notify, then all gen+notify) rather than calling `Self::wake` recursively. Avoid recursion: inline the two bumps.
  >
  > NOTE on `subscribe_guard_only`: `register(Some)` already incremented `subscribers` above, so it must obtain a guard WITHOUT a second increment. Refactor the existing `subscribe` into a private `make_guard(&self, key: Box<[u8]>) -> SubscriptionGuard` that only constructs the guard (registry Arc + key + notify), and call it from both `subscribe` (after its increment) and `register`. Do not double-count.

  (b) Add `mod wake;` and `pub use wake::{WakeSource, WakeRegistration};` to `crates/nexus-store/src/lib.rs` (place with the other `pub use` lines; keep `WakeReg` reachable via `nexus_store::notify::WakeReg`).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features testing wake:: notify`
Expected: PASS, including `armed_wait_never_loses_a_concurrent_wake` (50 iterations × multi-thread).

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/wake.rs crates/nexus-store/src/notify.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): WakeSource/WakeRegistration trait + in-process impl"
```

---

## Phase 2 — `nexus-store`: `Catchup` seam + generic live loop (additive)

Goal: write the single catch-up-then-tail loop, generic over a `Catchup`, returned as `impl Stream` (no `Box`). Test it against `InMemoryStore` (which gains a `WakeSource` impl). Nothing is wired into the public `Subscription` yet.

**Files:**
- Create: `crates/nexus-store/src/catchup.rs`
- Create: `crates/nexus-store/src/subscription_cursor.rs`
- Modify: `crates/nexus-store/src/testing.rs` (`InMemoryStore`: hold a `StreamNotifiers`, impl `WakeSource`, wake on append)
- Modify: `crates/nexus-store/src/lib.rs` (`mod catchup; mod subscription_cursor;`)

### Task 2.1: `InMemoryStore` gains a `WakeSource` impl and wakes on append

- [ ] **Step 1: Write the failing test** — in `crates/nexus-store/src/testing.rs` tests:

```rust
#[tokio::test]
async fn inmemory_wakes_registration_on_append() {
    use crate::wake::{WakeRegistration, WakeSource};
    use std::time::Duration;
    use tokio::time::timeout;
    let store = InMemoryStore::new();
    let reg = store.register(Some(b"s")).unwrap();
    let wait = reg.arm();
    // append one event to "s"
    let env = crate::envelope::pending_envelope(nexus::Version::INITIAL)
        .event_type("E").payload(b"x".to_vec()).unwrap().build();
    store.append(&TestId("s".into()), None, &[env]).await.unwrap();
    timeout(Duration::from_secs(5), wait).await.expect("append must wake the registration");
}
```

(Use the test `Id` already present in `testing.rs`; adapt `TestId` to the existing one.)

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features testing inmemory_wakes`
Expected: FAIL — `register` not implemented for `InMemoryStore`.

- [ ] **Step 3: Implement.** In `testing.rs`:
  - Add `notifiers: Arc<StreamNotifiers>` to `InMemoryStore` (init via `StreamNotifiers::new()`).
  - In `append`, after the in-memory write commits, call `self.notifiers.wake(id.as_ref())` (per-stream) and `self.notifiers.wake_all()`.
  - Impl `WakeSource for InMemoryStore` by delegating to the inner `notifiers` (`type Registration = WakeReg; type Error = NotifyError; register → notifiers.register; wake → notifiers.wake + wake_all`).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features testing inmemory_wakes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): InMemoryStore WakeSource impl + wake-on-append"
```

### Task 2.2: `Catchup` trait + per-stream / `$all` impls

- [ ] **Step 1: Write the failing test** — create `crates/nexus-store/src/catchup.rs`:

```rust
//! The store-side seam fusing a bounded position-keyed scan (`RawEventStore`)
//! with a wait (`WakeRegistration`) for ONE subscription target. Two
//! compile-time impls (per-stream, `$all`) let the single live loop in
//! [`crate::subscription_cursor`] monomorphize into two branch-free state
//! machines — no `dyn`, no boxing.

use core::future::Future;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::PersistedEnvelope;
use crate::store::{GlobalSeq, RawEventStore};
use crate::wake::{WakeRegistration, WakeSource};

pub(crate) trait Catchup: Send {
    type Position: Copy + Send;
    type Scan: futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send;
    type Error: core::error::Error + Send + Sync + 'static;

    fn read_after(&self, pos: Self::Position)
        -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;
    fn position_of(env: &PersistedEnvelope) -> Self::Position;
    fn start(from: Option<Self::Position>) -> Self::Position;
}
```

Add a per-stream impl (uses an owned id; reuse the existing fjall pattern — here store an `Arc<[u8]>` id and rebuild a thin `Id` wrapper, OR keep an `OwnedSubId` newtype):

```rust
pub(crate) struct StreamCatchup<S: RawEventStore + WakeSource> {
    store: Arc<S>,
    id: OwnedSubId,
    reg: <S as WakeSource>::Registration,
}

/// Owned id satisfying `Id`'s `'static` bound across reopened scans.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct OwnedSubId(Vec<u8>);
impl core::fmt::Display for OwnedSubId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match core::str::from_utf8(&self.0) { Ok(s) => f.write_str(s), Err(_) => write!(f, "<{} bytes>", self.0.len()) }
    }
}
impl AsRef<[u8]> for OwnedSubId { fn as_ref(&self) -> &[u8] { &self.0 } }
impl Id for OwnedSubId { const BYTE_LEN: usize = 0; }

impl<S: RawEventStore + WakeSource> Catchup for StreamCatchup<S> {
    type Position = Version;
    type Scan = <S as RawEventStore>::Stream;
    type Error = <S as RawEventStore>::Error;

    fn read_after(&self, pos: Version)
        -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send {
        self.store.read_stream(&self.id, pos)
    }
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static { self.reg.arm() }
    fn position_of(env: &PersistedEnvelope) -> Version { env.version() }
    fn start(from: Option<Version>) -> Version {
        from.map_or(Version::INITIAL, |v| v.next().unwrap_or(v))
    }
}
```

> NOTE: the `start` overflow case (`Some(Version::MAX)`) is handled in the loop by the first `read_after` returning an empty scan; full correctness (surfacing overflow as an error) is verified in Task 2.3's edge test. Keep `start` total here.

Add the `$all` impl analogously (`AllCatchup<S>`, `Position = GlobalSeq`, `Scan = S::AllStream`, `read_after → store.read_all`, `position_of → env.global_seq()`, `start → from.map_or(GlobalSeq::INITIAL, next)`).

A unit test that a `StreamCatchup` over `InMemoryStore` reads after a position:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    // construct StreamCatchup { store, id, reg } over InMemoryStore, append 3 events,
    // call read_after(Version::INITIAL), collect, assert versions [1,2,3].
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features testing catchup`
Expected: FAIL — module not declared / type errors until `mod catchup;` is added.

- [ ] **Step 3: Implement** — add `mod catchup;` to `lib.rs`; finish both `Catchup` impls and the constructor helpers (`StreamCatchup::new(store, id_bytes) -> Result<Self, S::Error>` calling `store.register(Some(id_bytes))`; `AllCatchup::new(store) -> Result<Self, S::Error>` calling `store.register(None)`). Map `WakeSource::Error` into the catchup error path as needed (the constructors return the wake error; the loop surfaces it — see Task 2.3).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features testing catchup`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/catchup.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): Catchup seam with per-stream and \$all impls"
```

### Task 2.3: The single generic live loop (`impl Stream`, no Box)

- [ ] **Step 1: Write the failing test** — create `crates/nexus-store/src/subscription_cursor.rs`:

```rust
//! The single catch-up-then-live-tail loop, generic over [`Catchup`].
//! Returned as `impl Stream` (RPIT) — no `Box<dyn>`. Holds the only copy of
//! the arm-before-confirm-rescan lost-wakeup discipline.

use crate::PersistedEnvelope;
use crate::catchup::Catchup;

/// Reopen granularity: drop+reopen the bounded scan every `CATCHUP_CHUNK`
/// rows during catch-up so a single adapter scan (and its GC watermark) is
/// never held across an unbounded backlog.
pub(crate) const CATCHUP_CHUNK: usize = 1024;

struct LiveState<C: Catchup> {
    c: C,
    pos: C::Position,
    scan: Option<C::Scan>,
    drained_in_chunk: usize,
}

pub(crate) fn live<C: Catchup + 'static>(
    c: C,
    from: Option<C::Position>,
) -> impl futures::Stream<Item = Result<PersistedEnvelope, C::Error>> + Send {
    let state = LiveState { pos: C::start(from), c, scan: None, drained_in_chunk: 0 };
    futures::stream::unfold(state, |mut s| async move {
        use futures::StreamExt;
        loop {
            // Ensure an open scan.
            if s.scan.is_none() {
                match s.c.read_after(s.pos).await {
                    Ok(scan) => { s.scan = Some(scan); s.drained_in_chunk = 0; }
                    Err(e) => return Some((Err(e), s)),
                }
            }
            let Some(scan) = s.scan.as_mut() else { continue };
            match scan.next().await {
                Some(Ok(env)) => {
                    s.pos = C::position_of(&env);
                    s.drained_in_chunk += 1;
                    if s.drained_in_chunk >= CATCHUP_CHUNK { s.scan = None; }
                    return Some((Ok(env), s));
                }
                Some(Err(e)) => { s.scan = None; return Some((Err(e), s)); }
                None => {
                    // Caught up: arm BEFORE the confirming re-scan.
                    s.scan = None;
                    let wait = s.c.arm();
                    match s.c.read_after(s.pos).await {
                        Ok(mut probe) => match probe.next().await {
                            Some(Ok(env)) => {
                                s.pos = C::position_of(&env);
                                s.scan = Some(probe);
                                s.drained_in_chunk = 1;
                                return Some((Ok(env), s));
                            }
                            Some(Err(e)) => return Some((Err(e), s)),
                            None => { drop(probe); wait.await; }
                        },
                        Err(e) => return Some((Err(e), s)),
                    }
                }
            }
        }
    })
}
```

Tests (drive via `InMemoryStore` + `StreamCatchup`):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    // 1. catch_up_yields_backlog_in_order: seed 5 events, live(StreamCatchup, None),
    //    take(5), assert versions [1..=5].
    // 2. live_tail_sees_post_subscribe_append: seed 1, open cursor, drain 1, then
    //    spawn an append of v2, assert next().await yields v2 (proves wake path).
    // 3. reopens_across_chunk_boundary: seed CATCHUP_CHUNK + 3 events, take(all),
    //    assert all present in order (proves the drop+reopen chunk logic).
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-store --features testing subscription_cursor`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement** — add `mod subscription_cursor;` to `lib.rs`; resolve clippy (the `let Some(..) = .. else { continue }` avoids `unwrap`; ensure no `#[allow]` in non-test code).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-store --features testing subscription_cursor`
Expected: PASS (all three tests, including the live-tail wake and chunk-boundary cases).

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/subscription_cursor.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): single generic catch-up-then-tail live loop (impl Stream)"
```

---

## Phase 3 — `nexus-fjall`: private `ScanStrategy`/`ScanCursor` over a lazy `Iter` (additive switch of read path)

Goal: collapse fjall's two bounded cursors into one private generic over a single lazy `fjall::Iter`; point `read_stream`/`read_all` at it; delete `stream.rs`/`all_stream.rs`; remove `batch_size`; add fjall's `WakeSource` impl. The old fjall subscription cursors keep working (they call `read_stream`/`read_all`, which now return the new `ScanCursor`).

**Files:**
- Create: `crates/nexus-fjall/src/scan.rs`
- Modify: `crates/nexus-fjall/src/store.rs` (read methods, `WakeSource` impl, drop `batch_size` field)
- Modify: `crates/nexus-fjall/src/builder.rs` (drop `batch_size`)
- Modify: `crates/nexus-fjall/src/lib.rs`
- Delete: `crates/nexus-fjall/src/stream.rs`, `crates/nexus-fjall/src/all_stream.rs`

### Task 3.1: `ScanStrategy` trait + `StreamScan`/`GlobalScan` impls

- [ ] **Step 1: Write the failing test** — create `crates/nexus-fjall/src/scan.rs` with the trait, the two impls, and unit tests that decode hand-built rows (port `decode_row_yields_envelope` and `decode_global_row_yields_envelope` + their rejection tests verbatim against `StreamScan::decode` / `GlobalScan::decode`):

```rust
//! fjall-private parameterization of the bounded keyset scan: the only parts
//! that differ between the per-stream (Version) and $all (GlobalSeq) reads —
//! key-byte bounds and row decode. NOT exported; no other adapter shares
//! fjall's on-disk key layout.

use arrayvec::ArrayString;
use bytes::Bytes;
use fjall::Slice;
use nexus::Version;
use nexus_store::{GlobalSeq, PersistedEnvelope};

use crate::error::FjallError;
use crate::subscription_id::OwnedStreamId; // OwnedStreamId moves here (see Task 3.3)

pub(crate) trait ScanStrategy: Send {
    type Position: Copy + Send + 'static;
    fn lower_key(&self, from: Self::Position) -> Vec<u8>;
    fn upper_key(&self) -> Vec<u8>;
    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError>;
}

pub(crate) struct StreamScan { pub(crate) id: OwnedStreamId, pub(crate) label: ArrayString<64> }
pub(crate) struct GlobalScan;

// StreamScan: Position = Version; lower/upper via encode_event_key; decode = old decode_row.
// GlobalScan: Position = GlobalSeq; lower/upper via encode_global_key(seq, 0|MAX); decode =
//   old decode_global_row (keeps the global_seq key-vs-frame cross-check).
```

Move the bodies of the existing `decode_row` (from `stream.rs`) and `decode_global_row` (from `all_stream.rs`) into the respective `decode` methods, and factor the shared `PersistedEnvelope::try_new` tail into a private `build_envelope(...)` helper in `scan.rs`.

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c cargo test -p nexus-fjall scan`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement** — declare `mod scan;` in `lib.rs`; complete both impls; keep `decode` error mapping identical to the originals (`CorruptValue`, `EnvelopeCorrupt`, global_seq-mismatch → `CorruptValue`).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop -c cargo test -p nexus-fjall scan`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/scan.rs crates/nexus-fjall/src/lib.rs crates/nexus-fjall/src/subscription_id.rs
git commit -m "feat(fjall): private ScanStrategy with StreamScan/GlobalScan impls"
```

### Task 3.2: `ScanCursor<S>` over a single lazy `fjall::Iter`

- [ ] **Step 1: Write the failing test** — add to `scan.rs`:

```rust
pub struct ScanCursor<S: ScanStrategy> {
    iter: fjall::Iter,
    strategy: S,
    poisoned: bool,
    #[cfg(debug_assertions)]
    prev: Option<u64>, // monotonicity assert via env.version()/global_seq() raw
}

impl<S: ScanStrategy> ScanCursor<S> {
    pub(crate) fn open(
        keyspace: &fjall::SingleWriterTxKeyspace,
        strategy: S,
        from: S::Position,
    ) -> Self {
        let lower = strategy.lower_key(from);
        let upper = strategy.upper_key();
        let iter = keyspace.inner().range(lower..=upper);
        Self { iter, strategy, poisoned: false,
               #[cfg(debug_assertions)] prev: None }
    }

    fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned { return None; }
        let guard = self.iter.next()?;
        let (key, value) = match guard.into_inner() {
            Ok(kv) => kv,
            Err(e) => { self.poisoned = true; return Some(Err(FjallError::Io(e))); }
        };
        match self.strategy.decode(&key, value) {
            Ok(env) => Some(Ok(env)),
            Err(e) => { self.poisoned = true; Some(Err(e)) }
        }
    }
}

impl<S: ScanStrategy> futures::Stream for ScanCursor<S> {
    type Item = Result<PersistedEnvelope, FjallError>;
    fn poll_next(mut self: core::pin::Pin<&mut Self>, _cx: &mut core::task::Context<'_>)
        -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.poll_one())
    }
}
```

Test: build a temp store via existing helpers, insert rows, open `ScanCursor::open(&events, StreamScan{..}, Version::INITIAL)`, collect, assert ordered versions. (Port the substance of the old `read_after_append_returns_events`.)

- [ ] **Step 2: Run to verify it fails** — `nix develop -c cargo test -p nexus-fjall scan_cursor` → FAIL (`ScanCursor` not yet complete / `fjall::Iter` import).

- [ ] **Step 3: Implement** — finish; confirm `fjall::Iter` is the owned type from `keyspace.inner().range(..)` (verified: `src/keyspace/mod.rs:444`, owned `Send + 'static`). Resolve the `Guard::into_inner` `Result` exactly as above.

- [ ] **Step 4: Run to verify it passes** — `nix develop -c cargo test -p nexus-fjall scan_cursor` → PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/scan.rs
git commit -m "feat(fjall): ScanCursor over a single lazy fjall::Iter"
```

### Task 3.3: Point read methods at `ScanCursor`; delete old cursors; remove `batch_size`

- [ ] **Step 1:** Move `OwnedStreamId` out of `subscription_stream.rs` into a small new `crates/nexus-fjall/src/subscription_id.rs` (`pub(crate)`), so both `scan.rs` and the (still-living) `subscription_stream.rs` import it from one place. `git add` the new file.

- [ ] **Step 2:** In `store.rs`, change the associated types and read methods:

```rust
type Stream = ScanCursor<StreamScan>;
type AllStream = ScanCursor<GlobalScan>;

async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
    Ok(ScanCursor::open(&self.events, StreamScan { id: OwnedStreamId::from_id(id), label: id.to_label() }, from))
}
async fn read_all(&self, from: GlobalSeq) -> Result<Self::AllStream, Self::Error> {
    Ok(ScanCursor::open(&self.events_global, GlobalScan, from))
}
```

- [ ] **Step 3:** Delete `crates/nexus-fjall/src/stream.rs` and `crates/nexus-fjall/src/all_stream.rs`; remove their `mod`/`pub use` lines from `lib.rs`. Remove the `batch_size: BatchSize` field from `FjallStore` and the `.batch_size(..)` setter + field from `builder.rs`; delete the now-unused `use nexus_store::batch::...` imports. Update the old `subscription_stream.rs`/`all_subscription_stream.rs` to drop the `batch_size` arg they passed and to call `read_stream`/`read_all` (now batch-free).

- [ ] **Step 4:** Update the in-`store.rs` tests that used `store_with_batch(n)` / `BatchSize` to use the default builder and seed `> CATCHUP_CHUNK` only where they were exercising refill cycles; the "pagination across refills" tests become plain full-read assertions (keep their `assert_eq!` on the full ordered vector).

- [ ] **Step 5: Run** — `nix develop -c cargo test -p nexus-fjall` → PASS (old subscription cursors still green via the new read path).

- [ ] **Step 6: Commit**

```bash
nix develop -c cargo fmt --all
git add -A crates/nexus-fjall
git commit -m "refactor(fjall): read paths use ScanCursor; delete bounded cursor dupes; drop batch_size"
```

### Task 3.4: fjall `WakeSource` impl

- [ ] **Step 1: Write the failing test** — in `store.rs` tests: register on the store, arm, append, assert the wait resolves within 5s (mirror Task 2.1 but against `FjallStore`).

- [ ] **Step 2: Run** → FAIL (`register` not impl'd for `FjallStore`).

- [ ] **Step 3: Implement** — `impl WakeSource for FjallStore` delegating to the held `Arc<StreamNotifiers>` (`type Registration = nexus_store::notify::WakeReg; type Error = NotifyError; register → self.notifiers.register; wake → self.notifiers.wake`). Ensure `append` already calls `self.notifiers.wake(id_bytes)` + `wake_all()` after commit (it does — keep it).

- [ ] **Step 4: Run** → PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): WakeSource impl delegating to StreamNotifiers"
```

---

## Phase 4 — Switchover: wire `Subscription` to the generic loop; delete the old subscription traits/cursors (atomic)

Goal: in one coherent change, replace the public `Subscription` plumbing with the generic loop and delete every old subscription path. Green before and after.

**Files:**
- Modify: `crates/nexus-store/src/subscription.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Delete: `crates/nexus-fjall/src/subscription_stream.rs`, `crates/nexus-fjall/src/all_subscription_stream.rs`
- Modify: `crates/nexus-fjall/src/store.rs` (remove `RawSubscription`/`RawAllSubscription` impls), `crates/nexus-fjall/src/lib.rs`

### Task 4.1: `Subscription<S>` builds the generic loop; delete the adapter subscription traits

- [ ] **Step 1: Move the test** — move the fjall subscription tests (`subscription_drains_many_batches_then_sees_live_event`, `subscribe_all_catches_up_then_sees_live_event`) into `nexus-store` `subscription.rs` tests, retargeted at `InMemoryStore` via the public `Subscription` API. Seed `> CATCHUP_CHUNK` where the original seeded "many batches".

- [ ] **Step 2: Run** → FAIL (new `Subscription::subscribe` signature not present yet).

- [ ] **Step 3: Implement** in `subscription.rs`:
  - Delete `pub trait RawSubscription`, `pub trait RawAllSubscription`, and the `sealed` module if nothing else uses it (it gates only these — confirm with `grep -rn sealed crates/`).
  - Rewrite the `Subscription<S>` impl blocks:

```rust
impl<S: RawEventStore + WakeSource> Subscription<S> {
    pub fn subscribe(
        &self, id: &impl Id, from: Option<Version>,
    ) -> impl futures::Stream<Item = Result<PersistedEnvelope, SubError<S>>> + Send + use<S> {
        // build StreamCatchup::new(Arc::clone(&self.store), id.as_ref()); on register
        // error, return a one-shot error stream; else live(catchup, from).
    }
    pub fn subscribe_all(
        &self, from: Option<GlobalSeq>,
    ) -> impl futures::Stream<Item = Result<PersistedEnvelope, SubError<S>>> + Send + use<S> {
        // AllCatchup::new(...) then live(..)
    }
}
```

  - Define the unified item error `SubError<S>` (an enum over `<S as RawEventStore>::Error` and `<S as WakeSource>::Error`) so register-failure surfaces in-band as the first `Err`. Use `thiserror` (project rule). Construct the register-error case via `futures::stream::once(async { Err(..) })` chained appropriately, or fold the register call into `Catchup::new` returning the error as the stream's first item.
  - Make `catchup`/`subscription_cursor` reachable from `subscription.rs` (they are `pub(crate)`).

- [ ] **Step 4:** In `nexus-fjall`: delete `subscription_stream.rs` + `all_subscription_stream.rs`; remove their `mod`/`pub use`; remove the `impl RawSubscription`/`RawAllSubscription for FjallStore` blocks and the `impl sealed::Sealed` line from `store.rs`. Replace the fjall subscription smoke tests with one integration test proving `Subscription::new(&Store::new(fjall)).subscribe(..)` yields catch-up + a live append (keep it small; the loop itself is tested in `nexus-store`).

- [ ] **Step 5: Run**

Run: `nix develop -c cargo test -p nexus-store --features testing subscription` and `nix develop -c cargo test -p nexus-fjall`
Expected: PASS in both.

- [ ] **Step 6: Commit**

```bash
nix develop -c cargo fmt --all
git add -A crates/nexus-store crates/nexus-fjall
git commit -m "refactor: Subscription builds the generic loop; delete RawSubscription + fjall subscription cursors"
```

### Task 4.2: Remove the now-dead `Notify` path from `StreamNotifiers`

- [ ] **Step 1:** `grep -rn "notifier()\|all_notifier()\|\.notify\b\|notify_waiters\|enable()" crates/` to confirm nothing outside `notify.rs` still uses the `Notify`-based API.

- [ ] **Step 2:** Delete `Entry.notify`, the `all: Arc<Notify>` field, `notifier()`, `all_notifier()`, and the `notify_waiters()` calls in `wake`/`wake_all` (keep only the `watch` generation bumps). Delete the old enable-based doc paragraph and the tests that asserted the `Notify`/`enable` behavior (`subscribe_then_wake_rouses_waiter`, `wake_all_rouses_all_waiter_not_per_stream_wake`, `concurrent_wake_is_not_lost`) — their coverage now lives in `wake.rs` (`armed_wait_never_loses_a_concurrent_wake`) and `subscription_cursor.rs`. Keep the lifecycle/reap tests (they still pass).

- [ ] **Step 3: Run** — `nix develop -c cargo test -p nexus-store --features testing notify wake` → PASS.

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/notify.rs
git commit -m "refactor(store): drop the dead Notify path; StreamNotifiers is watch-only"
```

---

## Phase 5 — `nexus-fjall`: append decomposition, arithmetic fixes, encoding cohesion (independent cleanups)

Each task here is self-contained and green on its own.

### Task 5.1: Decompose `append`; kill the `unwrap_or(u64::MAX)` sentinels

**Files:** Modify `crates/nexus-fjall/src/store.rs`.

- [ ] **Step 1:** Tests already cover `append` (conflict, sequential, multi-event, global index). Confirm they pass first: `nix develop -c cargo test -p nexus-fjall append`.

- [ ] **Step 2:** Extract three private methods on `FjallStore` (or free fns taking `&tx`): `read_current_version`, `check_optimistic`, `validate_sequential`. Replace enumerate-index arithmetic with running `u64` counters:

```rust
// version validation:
let mut expected_v = current_version.checked_add(1).ok_or(/* Conflict overflow */)?;
for env in envelopes {
    if env.version().as_u64() != expected_v { return Err(/* Conflict */); }
    expected_v = expected_v.checked_add(1).ok_or(/* Conflict overflow */)?;
}
// write loop:
let mut global_seq = current_global;
for env in envelopes {
    global_seq = global_seq.checked_add(1).ok_or(AppendError::Store(FjallError::GlobalSeqOverflow))?;
    // ... encode + insert events + events_global with `global_seq` ...
}
// counter advance: new value is simply `global_seq` (last assigned); no batch_len.
tx.insert(&self.global, GLOBAL_SEQ_KEY, global_seq.to_le_bytes());
```

Remove all three `u64::try_from(i).unwrap_or(u64::MAX)` and the `batch_len` line. Delete `#[allow(clippy::too_many_lines)]` once `append` is short enough; keep `#[allow(clippy::significant_drop_tightening)]` only if still flagged (tx held across the section — audit-grade reason).

- [ ] **Step 3: Run** — `nix develop -c cargo test -p nexus-fjall append` → PASS.

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/store.rs
git commit -m "refactor(fjall): decompose append; remove unwrap_or(u64::MAX) sentinels"
```

### Task 5.2: Remove the fake `DecodeError` sentinel in `decode_event_value`

**Files:** Modify `crates/nexus-fjall/src/encoding.rs` (→ `wire_key.rs` in 5.4).

- [ ] **Step 1:** Confirm `decode_event_value` tests pass: `nix develop -c cargo test -p nexus-fjall decode_event_value event_value`.

- [ ] **Step 2:** Delete the UTF-8 pre-check block (the `usize::try_from(...)` guards returning `InvalidSize { expected: usize::MAX, actual: 0 }` and the `std::str::from_utf8` call). Return the offsets straight from `wire::decode_frame`. (UTF-8 is validated by `PersistedEnvelope::try_new` downstream — the `decode_row` tests already assert corrupt rows are rejected.) If `DecodeError::InvalidUtf8` becomes unused, remove that variant too.

- [ ] **Step 3: Run** — `nix develop -c cargo test -p nexus-fjall` → PASS. Verify a corrupt-UTF-8 row is still rejected via the `decode_row`/`ScanStrategy::decode` path (the existing `decode_row_rejects_truncated_value` covers the corruption route; add one decoding a row whose event_type bytes are invalid UTF-8 and asserting `EnvelopeCorrupt`).

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/encoding.rs
git commit -m "fix(fjall): drop fake InvalidSize sentinel; rely on envelope UTF-8 validation"
```

### Task 5.3: Delete test-only `encode_event_value`; point tests/benches at `wire::encode_frame`

**Files:** Modify `crates/nexus-fjall/src/encoding.rs`, `benches/fjall_benchmarks.rs`, `tests/{property_tests,resilience_tests,snapshot_wire_format}.rs`, `src/scan.rs` tests.

- [ ] **Step 1:** Add a small `pub(crate)` test helper in a `#[cfg(test)]` module (e.g. in `scan.rs` tests) that wraps `wire::encode_frame` + the `nexus_store::value` newtypes to build a row `Slice` for unit tests — so in-crate `#[cfg(test)]` code has a one-liner without a production wrapper. (Production code must not gain test-only surface — `feedback_no_production_code_for_test_infra`.)

- [ ] **Step 2:** Delete `pub fn encode_event_value` from `encoding.rs`. Migrate every caller:
  - In-crate `#[cfg(test)]` callers → the Step 1 helper.
  - Integration tests (`tests/*.rs`) and benches → call `nexus_store::wire::encode_frame(global_seq, SchemaVersion::from_u32(..)?, &EventType::from_bytes(..)?, &Payload::from_bytes(..)?, meta.as_ref())` directly (the real production path), matching the signatures in `crates/nexus-store/src/wire.rs`. Benches now measure `encode_frame` (rule 8: benchmarks measure production code).

- [ ] **Step 3: Run** — `nix develop -c cargo test -p nexus-fjall --features testing,snapshot` and `nix develop -c cargo bench -p nexus-fjall --no-run` → both build/pass.

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add -A crates/nexus-fjall
git commit -m "refactor(fjall): remove test-only encode_event_value; tests/benches use wire::encode_frame"
```

### Task 5.4: Encoding cohesion (split snapshot codec) + fix `pub mod` leakage

**Files:** `crates/nexus-fjall/src/encoding.rs` → rename to `wire_key.rs`; Create `crates/nexus-fjall/src/snapshot.rs`; Modify `lib.rs`, `store.rs`.

- [ ] **Step 1:** `git mv crates/nexus-fjall/src/encoding.rs crates/nexus-fjall/src/wire_key.rs`. Move the snapshot codec (`SNAPSHOT_VALUE_HEADER_SIZE`, `encode_snapshot_value`, `decode_snapshot_value`, all `#[cfg(feature = "snapshot")]`) into a new `crates/nexus-fjall/src/snapshot.rs`. Update the `snapshot_impl` module in `store.rs` to import from `crate::snapshot`.

- [ ] **Step 2:** In `lib.rs`, change the module declarations from `pub mod {builder, encoding, error, store, stream, all_stream}` to private `mod` + a curated `pub use`. Final public surface: `FjallStore`, `FjallStoreBuilder`, `FjallError`, `KeyspaceConfig`, and the read-stream return types (`ScanCursor`, `StreamScan`, `GlobalScan` as needed by `RawEventStore::Stream`/`AllStream` being nameable). Keep `wire_key`/`snapshot` private (`mod`, no `pub`). Anything an integration test legitimately needs (key codecs for the conformance tests) gets a precise `pub use` or a `#[cfg(feature = "testing")]` re-export — verify against `tests/*.rs` imports and adjust.

- [ ] **Step 3: Run** — `nix develop -c cargo test -p nexus-fjall --features testing,snapshot` → PASS. Fix any integration-test import that reached into the old `pub mod encoding`.

- [ ] **Step 4: Commit**

```bash
nix develop -c cargo fmt --all
git add -A crates/nexus-fjall
git commit -m "refactor(fjall): split snapshot codec; rename encoding->wire_key; fix pub mod leakage"
```

---

## Phase 6 — Docs + final sweep

### Task 6.1: Update stale docs and the deviation log

- [ ] **Step 1:** Update `crates/nexus-store/src/subscription.rs` module + method docs that referenced the old `batch_size`-per-refill behavior and the deleted `RawSubscription` (lines ~156–158 and the adapter-authoring section). Update `crates/nexus-fjall/src/lib.rs` crate docs (remove the `Subscription`/`RawSubscription` impl claim; describe `RawEventStore + WakeSource` instead). Update the top-level `CLAUDE.md` store/fjall sections to reflect: loop in store, `WakeSource`, fjall has no subscription cursors, no `batch_size`.

- [ ] **Step 2:** Maintain a deviation log at `docs/plans/2026-06-23-nexus-fjall-cursor-unification-deviations.md` capturing any divergence from this plan with reason + impact (`feedback_deviation_log_during_refactors`). Do NOT log clippy-driven micro-changes (`feedback_clippy_compliance_not_a_deviation`).

- [ ] **Step 3: Commit**

```bash
nix develop -c cargo fmt --all
git add -A
git commit -m "docs: update subscription/fjall docs + CLAUDE.md for the extracted loop"
```

### Task 6.2: Open the PR

- [ ] **Step 1:** Push and open a PR against `main` with the `joeldsouzax` gh account (`feedback_gh_account_nexus`), squash-merge policy (`project_main_merge_policy`). Body: summarize the extraction (loop → store, zero-cost no-Box), the breaking contract change (`RawSubscription` removed; `Subscription::subscribe` now returns `impl Stream`), and the snapshot-consistency behavior change. Link the spec.

```bash
git push -u origin refactor/fjall-cursor-unification
gh pr create --title "refactor: extract subscription loop to nexus-store (zero-cost, no Box)" --body "<summary + spec link>"
```

---

## Self-review notes (author)

- **Spec coverage:** §3.1 store machinery → Phases 1,2,4; §3.2 fjall scan → Phase 3; §3.3 append → 5.1; §3.4 encoding → 5.2/5.3/5.4; §3.5 batch_size → 3.3; §4 behavior changes → 3.3 (snapshot), 3.3 (batch_size), 4.1 (traits), 1.x (StreamNotifiers); §6 risks → Phase ordering (StreamNotifiers first, isolated test 1.2). All covered.
- **Type consistency:** `WakeSource::register/wake`, `WakeRegistration::arm`, `Catchup::{read_after,arm,position_of,start}`, `ScanStrategy::{lower_key,upper_key,decode}`, `ScanCursor::open`, `live<C>(c, from)`, `CATCHUP_CHUNK` — names used identically across tasks.
- **Known soft spots to watch during execution (not placeholders, but judgment points):** (1) the `SubError<S>` enum shape in Task 4.1 — define it concretely with `thiserror` when implementing; (2) the `mark_unchanged`/`borrow_and_update` exact call in Task 1.2 `arm` — verify against the installed `tokio` `watch::Receiver` API and let `armed_wait_never_loses_a_concurrent_wake` be the proof; (3) Task 5.4 public-surface trimming must be reconciled against the actual `tests/*.rs` imports.
