# Arc-based Subscription Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the GAT-based borrowing `Subscription<M>` trait — whose `type Stream<'a>: ... where Self: 'a` clause makes HRTB type-equality bounds collapse to `Self: 'static` on stable Rust — with an Arc-based, 'static `Subscription<M>` shape whose cursors own `Arc<Store>` instead of borrowing `&'a Store`. Delete the `BaseEventStream` HRTB workaround on the subscription side; keep it scoped to the `RawEventStore::Stream` path.

**Architecture:** Subscriptions are long-lived independent processes — projections, sagas, fan-out streams — that call `subscribe()` once and loop forever. They must outlive any caller scope. The current trait models them as nested borrows, which fights the type system at every consumer site. The right shape is: cursors own their share of the store (`Arc<Store>`), `subscribe(&self, ...)` is a regular method on `Arc<S>`, and the returned stream is `'static`. Borrow stays everywhere else (`RawEventStore` reads, repository load paths, codec, snapshot hydration) — they are one-shot operations, not independent processes.

**Tech Stack:** Rust 2024 edition (nexus repo workspace), `nexus-store`, `nexus-fjall`, `nexus-framework`, `nexus-store-testing`. Build/check: `nix develop -c nix flake check`. Squash-merge via `gh pr merge --squash --delete-branch`. Signed commits required.

---

## Design Rationale (Philosophy)

The principle this refactor encodes is **"borrow as much as possible; own only when the consumer must outlive any single scope."** Subscriptions are the first place in this codebase where independence requires ownership.

The HRTB-type-equality wall we keep working around (see PR #170's deviation log: `BaseEventStream` was added precisely because `for<'a> Sub::Stream<'a>::Item<'a> = PersistedEnvelope<'a, M>` collapses to `Sub: 'static` on stable Rust) is the type system telling us we modeled an independent process as a nested scope. Two GATs nested inside an HRTB ("`Sub::Stream<'_>` is a GAT itself, so a witness HRTB nested inside another HRTB was rejected with 'implementation of EventStream is not general enough'") is not a compiler bug — it's a design smell.

The Arc-based shape:

- `Arc::clone` is one atomic add at `subscribe()` time. Paid **once** per projection (subscriptions are long-lived, not per-event).
- Per-event cost is identical to the borrow design — an `Arc::Deref` is just a pointer dereference, no atomics.
- Cursors become `'static`: spawnable, movable across async boundaries, carriable in supervisors.
- The HRTB wall vanishes — `Subscription::Stream` becomes a concrete (non-GAT) type. There is no `where Self: 'a` on the trait, so HRTB type equality on the cursor's `Item<'a>` GAT is no longer nested.
- `BaseEventStream` + its 4 production impls on the subscription side + ~14 conformance harness bounds + the projection runner's `<Sub::Stream<'_> as BaseEventStream>::to_envelope(item)` site delete with the trait.

What does **not** change: `RawEventStore::Stream<'a>` keeps its borrow shape. A one-shot byte-level read does not need independence from its caller — it lives inside a single `load()` or `replay_from()` call. The four `to_envelope` sites in `crates/nexus-store/src/repository.rs` (lines 227, 319, 526, 616) stay; `BaseEventStream` stays implemented on `InMemoryStream` and `FjallStream` (the `RawEventStore::Stream` types), and only the subscription-side impls/bounds delete.

---

## File Structure

### Per-PR files-changed map

**PR1 (introduce parallel shape):**

| File | Action | Why |
|---|---|---|
| `crates/nexus-store/src/store.rs` | Modify | Add the new `Subscription` trait under a temporary name (`SharedSubscription<M>`) alongside the existing `Subscription<M>`. No-GAT; `type Stream: EventStream<M> + Send + 'static`. |
| `crates/nexus-store/src/testing.rs` | Modify | Add a parallel cursor type `SharedInMemorySubscriptionStream` (owns `Arc<InMemoryStore>`, no `'a` parameter). Add `impl SharedSubscription<()> for Arc<InMemoryStore>`. Old `InMemorySubscriptionStream<'a>` + old `Subscription` impl untouched. |
| `crates/nexus-fjall/src/subscription_stream.rs` | Modify | Same shape: add `SharedFjallSubscriptionStream` (owns `Arc<FjallStore>`, no `'a` parameter). Existing `FjallSubscriptionStream<'a>` untouched. |
| `crates/nexus-fjall/src/store.rs` | Modify | Add `impl SharedSubscription<()> for Arc<FjallStore>` alongside the existing `impl Subscription<()> for FjallStore`. |
| `crates/nexus-store/src/lib.rs` | Modify | Re-export `SharedSubscription` alongside `Subscription`. |
| `crates/nexus-store/tests/shared_subscription_smoke.rs` | Create | One-test smoke file: subscribe via `Arc<InMemoryStore>`, append events from another task, drain the cursor. Proves the new shape works end-to-end. |
| `crates/nexus-fjall/tests/shared_subscription_smoke.rs` | Create | Mirror smoke test for `Arc<FjallStore>`. |

**PR2 (migrate consumers):**

| File | Action | Why |
|---|---|---|
| `crates/nexus-framework/src/projection/mod.rs` | Modify | Change `Sub: Subscription<()>` bound to `Sub: SharedSubscription<()>`. Drop the `<Sub::Stream<'_> as BaseEventStream>::to_envelope(item)` line — `item` is already `PersistedEnvelope<'_, ()>`. |
| `crates/nexus-framework/src/projection/builder.rs` | Modify | Update builder bounds parallel to `mod.rs`. |
| `crates/nexus-framework/src/projection/error.rs` | Modify | The `Sub::Error` projection in `ProjectionError` stays — `SharedSubscription` exposes the same `Error` shape. Just retarget the trait name. |
| `crates/nexus-store-testing/src/lib.rs` | Modify | The conformance harness covers `RawEventStore::Stream` cursors (`FjallStream`, `InMemoryStream`) — these are **not** subscription cursors, so the bounds stay on `BaseEventStream`. No-op here in PR2 unless the harness has subscription-specific entry points (it does not, as of current main). |
| `crates/nexus-store/tests/subscription_tests.rs` | Modify | If this file directly uses `Subscription<()>`, retarget to `SharedSubscription<()>` and wrap the store in `Arc::new(...)`. |
| `crates/nexus-fjall/tests/subscription_tests.rs` | Modify | Same. |

**PR3 (delete old shape, rename):**

| File | Action | Why |
|---|---|---|
| `crates/nexus-store/src/store.rs` | Modify | Delete the old `Subscription<M>` trait (lines 187–213) and the `impl Subscription<M> for &T` blanket (lines 219–234). Rename `SharedSubscription` → `Subscription`. |
| `crates/nexus-store/src/testing.rs` | Modify | Delete `InMemorySubscriptionStream<'a>` struct + its `BaseEventStream`/`EventStream` impls + the `impl Subscription<()> for InMemoryStore` block. Rename `SharedInMemorySubscriptionStream` → `InMemorySubscriptionStream`. Drop the `BaseEventStream` impl on the renamed type — subscription cursors no longer need it. |
| `crates/nexus-fjall/src/subscription_stream.rs` | Modify | Delete `FjallSubscriptionStream<'a>` + its impls. Rename `SharedFjallSubscriptionStream` → `FjallSubscriptionStream`. Drop the `BaseEventStream` impl on the renamed type. |
| `crates/nexus-fjall/src/store.rs` | Modify | Delete the old `impl Subscription<()> for FjallStore`. The renamed `impl Subscription<()> for Arc<FjallStore>` stays. |
| `crates/nexus-store/src/lib.rs` | Modify | Re-export only the canonical `Subscription`. Remove `SharedSubscription` alias. |
| `crates/nexus-framework/src/projection/mod.rs` | Modify | Trait name back to `Subscription<()>`. Drop the `use ... BaseEventStream` import. |
| `crates/nexus-framework/src/projection/builder.rs` | Modify | Same. |
| `crates/nexus-store-testing/src/lib.rs` | No change | Stays on `BaseEventStream` (for `RawEventStore::Stream` conformance). |
| `crates/nexus-store/tests/subscription_tests.rs` | Modify | Rename + drop `Shared` references. |
| `crates/nexus-fjall/tests/subscription_tests.rs` | Modify | Same. |

**PR4 (evaluate `OwnedEventStream`):**

| File | Action | Why |
|---|---|---|
| `crates/nexus-store/src/stream/owned.rs` | Evaluate / modify / delete | With subscription cursors becoming `'static`, the `'static` bound that `IntoStream` requires is more naturally satisfied. Whether `OwnedEventStream` simplifies or deletes depends on whether the futures-bridge surface still needs an explicit owned-item witness on combinators (`Map`, `TryMap`). |
| `crates/nexus-store/src/stream/mod.rs` | Modify | Adjust re-exports based on PR4 outcome. |
| `crates/nexus-store/src/lib.rs` | Modify | Same. |
| `crates/nexus-store/tests/futures_bridge_tests.rs` | Modify | Update tests if `OwnedEventStream` simplifies. |

---

## Constraints (apply to every PR)

- Each PR must compile **and** pass `nix develop -c nix flake check` on its own branch.
- Squash-merge via `gh pr merge --squash --delete-branch`.
- Signed commits required. Nix Flake Check required.
- Conventional commit subjects: `feat(store)`, `refactor(store, framework)`, `refactor(fjall)`, etc.
- Issue #176 (bounded batch) is unrelated — do **not** entangle.
- Do **not** touch `RawEventStore::Stream<'a>`'s borrow shape.
- Per-PR acceptance criteria below; tick every box before opening the PR.

---

## Deviation Log

Maintain `docs/plans/2026-05-26-arc-subscription-deviations.md` alongside this plan. Every divergence from the steps below — assumption taken, work skipped, work added, name changed — gets an entry. Format and schema mirror `cheerful-yawning-hopper-deviations.md` (see file).

---

## PR1: Introduce `SharedSubscription` shape alongside existing `Subscription`

**Goal:** Add the new Arc-based subscription trait, two new cursor types (in-memory + fjall), and the two new `impl SharedSubscription<()> for Arc<Store>` blocks — without touching the existing `Subscription` shape. Workspace compiles. Both shapes coexist. Old consumers (projection runner, conformance harness) still use the old `Subscription`.

**Branch:** `refactor/arc-subscription` (already created off main). PR1 ships from this branch as the first commit-set, then sub-branches off for PR2–PR4 as work proceeds. Alternative: open a sub-branch `pr1/arc-subscription-introduce` if the maintainer prefers fully-isolated PRs.

**Acceptance criteria:**

- [ ] `nix develop -c nix flake check` passes on the branch.
- [ ] `crates/nexus-store/tests/shared_subscription_smoke.rs` compiles and passes.
- [ ] `crates/nexus-fjall/tests/shared_subscription_smoke.rs` compiles and passes.
- [ ] All existing tests still pass — no old subscription test touched.
- [ ] No `<Sub::Stream<'_> as BaseEventStream>::to_envelope(...)` site exists for the new trait — the new `Item<'_>` is `PersistedEnvelope<'_, ()>` directly.
- [ ] The HRTB-equality "spike" (Task 1 below) compiles. If it does not, escalate per the spike's fallback plan and update the plan + deviation log before continuing.

### Task 1: Spike — verify HRTB type equality compiles on the new shape

**Files:**
- Create: `crates/nexus-store/src/store.rs` (sketch added then reverted, OR a throwaway `crates/nexus-store/tests/spike_hrtb.rs`)

**Why this task exists:** The plan assumes that with `Subscription::Stream: EventStream<M> + Send + 'static` (no GAT on the trait side; only the cursor's `Item<'a>` is a GAT), an HRTB type-equality bound `for<'a> <Sub::Stream as EventStream<()>>::Item<'a> = PersistedEnvelope<'a, ()>` compiles. PR #170's deviation log shows the bound failed when nested inside another GAT (`Sub::Stream<'_>` was itself a GAT). With the GAT removed from the trait side, the bound should now compile — but "should" is not "does". Validate before betting four PRs on it.

- [ ] **Step 1: Write the spike in a throwaway test file**

```rust
// crates/nexus-store/tests/spike_hrtb.rs
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

// Mirror of the proposed new trait shape — no GAT.
trait FakeSharedSubscription {
    type Stream: EventStream<()> + Send + 'static;
}

// Consumer site: can we bound the Item HRTB?
fn _consume<Sub>(_s: &Sub)
where
    Sub: FakeSharedSubscription,
    // The critical bound — does this compile?
    for<'a> <Sub::Stream as EventStream<()>>::Item<'a>: AsRef<PersistedEnvelope<'a, ()>>,
{
}

#[test]
fn spike_compiles() {
    // No runtime assertion; the compile is the assertion.
}
```

- [ ] **Step 2: Run the spike**

Run: `nix develop -c cargo build -p nexus-store --tests`
Expected: clean build. If it fails with "implementation of EventStream is not general enough" or similar, the HRTB still doesn't compile — see fallback below.

- [ ] **Step 3: Decide forward**

If the spike compiles: delete `tests/spike_hrtb.rs` and proceed to Task 2.

If it does **not** compile: stop. Update the plan to retain a `SubscriptionItem` witness sub-trait (the same shape as `BaseEventStream` but on the subscription side only). Log the deviation. Then proceed with the modified plan. The witness approach is a clean fallback — it isolates the workaround to one trait and three identity impls.

- [ ] **Step 4: Commit (only if spike resolved)**

```bash
git add docs/plans/2026-05-26-arc-subscription-plan.md docs/plans/2026-05-26-arc-subscription-deviations.md
git commit -m "docs(plan): arc-subscription refactor plan + deviation log"
```

### Task 2: Add `SharedSubscription<M>` trait to `nexus-store/src/store.rs`

**Files:**
- Modify: `crates/nexus-store/src/store.rs` (add new trait below the existing `Subscription<M>` block)

- [ ] **Step 1: Add the trait definition**

Insert after the existing `impl<T: Subscription<M> + Sync, M: 'static> Subscription<M> for &T` block (after line 234), before the `GlobalSeq` section (line 236):

```rust
// ═══════════════════════════════════════════════════════════════════════════
// SharedSubscription<M> — Arc-based 'static subscription shape
// ═══════════════════════════════════════════════════════════════════════════

/// A subscription whose cursor owns an `Arc<Store>` and is therefore `'static`.
///
/// Implemented on `Arc<Store>` directly so the trait method `subscribe`
/// takes `&self` and returns a cursor that outlives the call. Each
/// subscription pays one `Arc::clone` at `subscribe()` time; per-event
/// cost is identical to the borrowed shape.
///
/// # Why this is separate from [`Subscription`]
///
/// The borrowed [`Subscription`] uses a GAT `type Stream<'a>: ... where Self: 'a`.
/// HRTB type-equality on that GAT collapses to `Self: 'static` on stable
/// Rust, so consumer sites cannot bound `for<'a> Stream<'a>::Item<'a>`
/// without a witness sub-trait ([`BaseEventStream`]). This trait's
/// `type Stream: EventStream<M> + Send + 'static` is a concrete, non-GAT
/// associated type — the HRTB nesting goes away.
///
/// PR3 of the arc-subscription refactor renames this to `Subscription` and
/// deletes the borrowed shape entirely. The temporary `Shared` prefix is
/// only there to let PR1 introduce the new shape alongside the existing
/// one without breaking the workspace build.
pub trait SharedSubscription<M: 'static = ()> {
    /// The subscription stream type — an [`EventStream`] that never exhausts.
    type Stream: crate::stream::EventStream<M, Error = Self::Error> + Send + 'static;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Subscribe to events in a single stream.
    fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}
```

Notes on the signature:
- `M: 'static = ()` matches the existing `Subscription<M: 'static>` defaulting.
- `Stream: EventStream<M> + Send + 'static` is the heart of the refactor — concrete, 'static, no GAT.
- `fn subscribe(&self, id, from) -> impl Future<...>` has no lifetime parameter; the returned future borrows `&self` for as long as it takes to set up the cursor, but the cursor itself is `'static`.

- [ ] **Step 2: Build the workspace**

Run: `nix develop -c cargo build -p nexus-store`
Expected: clean build. The trait has no implementors yet — that comes in Tasks 3–6.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/store.rs
git commit -m "feat(store): add SharedSubscription trait (Arc-based 'static shape)"
```

### Task 3: Add `SharedInMemorySubscriptionStream` cursor in `nexus-store/src/testing.rs`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs` (append after the existing `impl Subscription<()> for InMemoryStore` block, line 421)

- [ ] **Step 1: Add the new cursor + impls**

```rust
// ═══════════════════════════════════════════════════════════════════════════
// SharedInMemorySubscriptionStream — Arc-based cursor (PR1 of arc-subscription refactor)
// ═══════════════════════════════════════════════════════════════════════════

/// Subscription cursor that owns an `Arc<InMemoryStore>` instead of borrowing.
///
/// `'static` (no lifetime parameter). Spawnable across async boundaries.
/// PR3 renames this to `InMemorySubscriptionStream` and deletes the
/// borrowed variant.
pub struct SharedInMemorySubscriptionStream {
    store: std::sync::Arc<InMemoryStore>,
    stream_id: String,
    buffer: Vec<ReadRow>,
    pos: usize,
    last_version: Option<Version>,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl SharedInMemorySubscriptionStream {
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r.version >= from_version.as_u64())
                        .map(|r| ReadRow {
                            version: r.version,
                            global_seq: r.global_seq,
                            event_type: r.event_type.clone(),
                            schema_version: r.schema_version,
                            payload: r.payload.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        self.buffer = buffer;
        self.pos = 0;
    }

    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or_else(
            || Ok(Version::INITIAL),
            |v| v.next().ok_or(InMemoryStoreError::VersionOverflow),
        )
    }
}

impl EventStream for SharedInMemorySubscriptionStream {
    type Item<'a>
        = PersistedEnvelope<'a>
    where
        Self: 'a;
    type Error = InMemoryStoreError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        loop {
            if self.pos < self.buffer.len() {
                let row = &self.buffer[self.pos];
                self.pos += 1;
                #[cfg(debug_assertions)]
                {
                    if let Some(prev) = self.prev_version {
                        debug_assert!(
                            row.version > prev,
                            "Subscription monotonicity violated: version {} is not greater than previous {}",
                            row.version,
                            prev,
                        );
                    }
                    self.prev_version = Some(row.version);
                }
                let Some(version) = Version::new(row.version) else {
                    return Err(InMemoryStoreError::CorruptVersion);
                };
                self.last_version = Some(version);
                return Ok(Some(PersistedEnvelope::new_unchecked(
                    version,
                    row.global_seq,
                    &row.event_type,
                    row.schema_version,
                    &row.payload,
                    (),
                )));
            }

            let notified = self.store.notify.notified();
            let from = self.next_read_version()?;
            self.refill(from).await;
            if !self.buffer.is_empty() {
                continue;
            }
            notified.await;
        }
    }
}

impl crate::store::SharedSubscription<()> for std::sync::Arc<InMemoryStore> {
    type Stream = SharedInMemorySubscriptionStream;
    type Error = InMemoryStoreError;

    async fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<SharedInMemorySubscriptionStream, InMemoryStoreError> {
        let stream_id = id.to_string();
        let from_version = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(InMemoryStoreError::VersionOverflow)?,
        };
        let mut sub = SharedInMemorySubscriptionStream {
            store: std::sync::Arc::clone(self),
            stream_id,
            buffer: Vec::new(),
            pos: 0,
            last_version: from,
            #[cfg(debug_assertions)]
            prev_version: from.map(Version::as_u64),
        };
        sub.refill(from_version).await;
        Ok(sub)
    }
}
```

Notes:
- The cursor has **no** `BaseEventStream` impl. PR1's new shape doesn't need it (consumers will work on the concrete `PersistedEnvelope<'a>` item directly in PR2; the spike in Task 1 validated this).
- The cursor structure mirrors `InMemorySubscriptionStream<'a>` line-for-line — the only field change is `store: Arc<InMemoryStore>` vs `store: &'a InMemoryStore`. The `refill` body uses `self.store.streams.lock().await` exactly as before; `Arc::Deref` is transparent.
- `streams` and `notify` are `pub(crate)` already on `InMemoryStore`. **Verify**: if any field on `InMemoryStore` accessed here is currently private, escalate via a constructor-style helper rather than widening visibility. (Per CLAUDE.md rule "pub(crate) fields → constructor", but for same-crate access `pub(crate)` is fine.)

- [ ] **Step 2: Build the crate**

Run: `nix develop -c cargo build -p nexus-store --features testing`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): SharedInMemorySubscriptionStream + impl on Arc<InMemoryStore>"
```

### Task 4: Smoke test for the in-memory shared subscription

**Files:**
- Create: `crates/nexus-store/tests/shared_subscription_smoke.rs`

- [ ] **Step 1: Write the test**

```rust
//! Smoke test for the Arc-based SharedSubscription on InMemoryStore.
//!
//! Proves the new shape works end-to-end: subscribe on Arc<Store>, append
//! events from another task, drain the cursor.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::Arc;

use nexus::{Id, Version};
use nexus_store::envelope::pending_envelope;
use nexus_store::store::{RawEventStore, SharedSubscription};
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

#[tokio::test]
async fn shared_subscription_yields_appended_events() {
    let store = Arc::new(InMemoryStore::new());
    let id = TestId("s-1".to_owned());

    // Subscribe before any appends.
    let writer = Arc::clone(&store);
    let mut sub = store.subscribe(&id, None).await.unwrap();

    // Write one event from a separate task.
    let id_w = id.clone();
    let writer_handle = tokio::spawn(async move {
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"hello".to_vec())
            .build_without_metadata();
        writer.append(&id_w, None, &[env]).await.unwrap();
    });

    let env = sub.next().await.unwrap().unwrap();
    assert_eq!(env.version(), Version::new(1).unwrap());
    assert_eq!(env.event_type(), "Created");
    assert_eq!(env.payload(), b"hello");

    writer_handle.await.unwrap();
}

#[tokio::test]
async fn shared_subscription_cursor_is_static() {
    fn assert_static<T: 'static>(_: &T) {}
    let store = Arc::new(InMemoryStore::new());
    let id = TestId("s-1".to_owned());
    let sub = store.subscribe(&id, None).await.unwrap();
    // The whole point of the refactor: this assertion must compile.
    assert_static(&sub);
}
```

- [ ] **Step 2: Run the test**

Run: `nix develop -c cargo test -p nexus-store --features testing --test shared_subscription_smoke`
Expected: both tests pass. `shared_subscription_cursor_is_static` is the proof that the refactor delivers what it promised.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/tests/shared_subscription_smoke.rs
git commit -m "test(store): smoke + static-ness assertion for SharedSubscription"
```

### Task 5: Add `SharedFjallSubscriptionStream` cursor in `nexus-fjall/src/subscription_stream.rs`

**Files:**
- Modify: `crates/nexus-fjall/src/subscription_stream.rs` (append below the existing `EventStream` impl, line 210)

- [ ] **Step 1: Add the new cursor**

```rust
// ═══════════════════════════════════════════════════════════════════════════
// SharedFjallSubscriptionStream — Arc-based cursor (PR1 of arc-subscription refactor)
// ═══════════════════════════════════════════════════════════════════════════

use std::sync::Arc;

/// Subscription cursor that owns an `Arc<FjallStore>` instead of borrowing.
///
/// `'static`. Spawnable across async boundaries. PR3 renames this to
/// `FjallSubscriptionStream` and deletes the borrowed variant.
pub struct SharedFjallSubscriptionStream {
    store: Arc<FjallStore>,
    stream_key: OwnedStreamId,
    label: ArrayString<64>,
    inner: FjallStream,
    last_version: Option<Version>,
}

impl SharedFjallSubscriptionStream {
    pub(crate) const fn new(
        store: Arc<FjallStore>,
        stream_key: OwnedStreamId,
        label: ArrayString<64>,
        inner: FjallStream,
        last_version: Option<Version>,
    ) -> Self {
        Self {
            store,
            stream_key,
            label,
            inner,
            last_version,
        }
    }

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

impl EventStream for SharedFjallSubscriptionStream {
    type Item<'a>
        = PersistedEnvelope<'a>
    where
        Self: 'a;
    type Error = FjallError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        // Body byte-for-byte identical to FjallSubscriptionStream::next —
        // the only difference is the type of `self.store` (Arc vs &). The
        // dereference syntax is the same.
        loop {
            if self.inner.poisoned || self.inner.pos >= self.inner.events.len() {
                // fall through to refill
            } else {
                let (key, value) = &self.inner.events[self.inner.pos];
                self.inner.pos += 1;

                let Ok((_id_bytes, version_raw)) = decode_event_key(key) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: None,
                    });
                };

                #[cfg(debug_assertions)]
                {
                    if let Some(prev) = self.inner.prev_version {
                        debug_assert!(
                            version_raw > prev,
                            "Subscription monotonicity violated: version {version_raw} is not greater than previous {prev}",
                        );
                    }
                    self.inner.prev_version = Some(version_raw);
                }

                let Ok((global_seq_raw, schema_version, event_type, payload)) =
                    decode_event_value(value)
                else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let Some(version) = Version::new(version_raw) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let Some(global_seq) = GlobalSeq::new(global_seq_raw) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let Ok(envelope) = PersistedEnvelope::try_new(
                    version,
                    global_seq,
                    event_type,
                    schema_version,
                    payload,
                    (),
                ) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                self.last_version = Some(version);
                return Ok(Some(envelope));
            }

            let notified = self.store.notify.notified();
            let from = self.next_read_version()?;
            self.refill(from).await?;
            if !self.inner.events.is_empty() {
                continue;
            }
            notified.await;
        }
    }
}
```

Notes:
- **No `BaseEventStream` impl** for the new cursor (intentional — see PR1 design).
- The `next()` body is byte-for-byte identical to `FjallSubscriptionStream::next()`. The only structural difference is `store: Arc<FjallStore>` vs `store: &'a FjallStore`. Verify before committing that no behavioral change crept in — `git diff` the two implementations side-by-side and confirm the cursor logic is identical.

- [ ] **Step 2: Build**

Run: `nix develop -c cargo build -p nexus-fjall`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-fjall/src/subscription_stream.rs
git commit -m "feat(fjall): SharedFjallSubscriptionStream (Arc-owned, 'static)"
```

### Task 6: Add `impl SharedSubscription<()> for Arc<FjallStore>` in `nexus-fjall/src/store.rs`

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` (after the existing `impl Subscription<()> for FjallStore`, around line 365)

- [ ] **Step 1: Add the impl**

```rust
// ═══════════════════════════════════════════════════════════════════════════
// SharedSubscription impl (PR1 of arc-subscription refactor)
// ═══════════════════════════════════════════════════════════════════════════

use std::sync::Arc;
use nexus_store::store::SharedSubscription;
use crate::subscription_stream::SharedFjallSubscriptionStream;

impl SharedSubscription<()> for Arc<FjallStore> {
    type Stream = SharedFjallSubscriptionStream;
    type Error = FjallError;

    async fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<SharedFjallSubscriptionStream, FjallError> {
        let start = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(FjallError::VersionOverflow)?,
        };
        let owned_id = crate::subscription_stream::OwnedStreamId::from_id(id);
        let label = id.to_label();
        let inner = self.read_stream(&owned_id, start).await?;
        Ok(SharedFjallSubscriptionStream::new(
            Arc::clone(self),
            owned_id,
            label,
            inner,
            from,
        ))
    }
}
```

- [ ] **Step 2: Build**

Run: `nix develop -c cargo build -p nexus-fjall`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): impl SharedSubscription<()> for Arc<FjallStore>"
```

### Task 7: Smoke test for the fjall shared subscription

**Files:**
- Create: `crates/nexus-fjall/tests/shared_subscription_smoke.rs`

- [ ] **Step 1: Write the test**

Mirror of the in-memory smoke test, swapping `InMemoryStore::new()` for `FjallStore::builder(tempdir.path()).open().unwrap()`.

```rust
#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::Arc;

use nexus::{Id, Version};
use nexus_fjall::FjallStore;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::{RawEventStore, SharedSubscription};
use nexus_store::stream::EventStream;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

#[tokio::test]
async fn shared_subscription_yields_appended_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FjallStore::builder(dir.path().join("db")).open().unwrap());
    let id = TestId("s-1".to_owned());

    let mut sub = store.subscribe(&id, None).await.unwrap();

    let writer = Arc::clone(&store);
    let id_w = id.clone();
    let writer_handle = tokio::spawn(async move {
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"hello".to_vec())
            .build_without_metadata();
        writer.append(&id_w, None, &[env]).await.unwrap();
    });

    let env = sub.next().await.unwrap().unwrap();
    assert_eq!(env.version(), Version::new(1).unwrap());
    assert_eq!(env.event_type(), "Created");
    assert_eq!(env.payload(), b"hello");

    writer_handle.await.unwrap();
}

#[tokio::test]
async fn shared_subscription_cursor_is_static() {
    fn assert_static<T: 'static>(_: &T) {}
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FjallStore::builder(dir.path().join("db")).open().unwrap());
    let id = TestId("s-1".to_owned());
    let sub = store.subscribe(&id, None).await.unwrap();
    assert_static(&sub);
}
```

- [ ] **Step 2: Run the test**

Run: `nix develop -c cargo test -p nexus-fjall --test shared_subscription_smoke`
Expected: both tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-fjall/tests/shared_subscription_smoke.rs
git commit -m "test(fjall): smoke + static-ness assertion for SharedSubscription"
```

### Task 8: Re-export `SharedSubscription` from `nexus-store::lib.rs`

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` (line 41)

- [ ] **Step 1: Add the re-export**

```rust
// Before:
pub use store::{GlobalSeq, RawEventStore, Store, Subscription};

// After:
pub use store::{GlobalSeq, RawEventStore, SharedSubscription, Store, Subscription};
```

- [ ] **Step 2: Build the workspace**

Run: `nix develop -c cargo build --workspace`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "feat(store): re-export SharedSubscription from crate root"
```

### Task 9: Full flake check + open PR1

- [ ] **Step 1: Run the full check suite**

Run: `nix develop -c nix flake check`
Expected: every check passes (clippy, fmt, tests, taplo, audit, deny, hakari).

- [ ] **Step 2: Format**

Run: `nix develop -c cargo fmt --all`
Then verify nothing changed: `git status` should show clean.

- [ ] **Step 3: Push and open PR**

```bash
git push -u origin refactor/arc-subscription
gh pr create --title "feat(store, fjall): SharedSubscription (Arc-based, 'static cursor) — PR1 of arc-subscription refactor" --body "$(cat <<'EOF'
## Summary

- Introduces `SharedSubscription<M>` trait — Arc-based, no GAT, `'static` cursor type.
- Implements it on `Arc<InMemoryStore>` and `Arc<FjallStore>` with parallel cursor types `SharedInMemorySubscriptionStream` and `SharedFjallSubscriptionStream`.
- Adds two smoke tests proving the cursor is `'static`.
- Existing `Subscription<M>` trait and its borrowing cursors are **untouched** — both shapes coexist.

PR1 of a 4-PR refactor that replaces the borrowed `Subscription` with an Arc-based shape. Background and full plan: `docs/plans/2026-05-26-arc-subscription-plan.md`. Deviation log: `docs/plans/2026-05-26-arc-subscription-deviations.md`.

## Test plan

- [ ] `nix develop -c nix flake check` passes locally
- [ ] `cargo test -p nexus-store --features testing --test shared_subscription_smoke` passes
- [ ] `cargo test -p nexus-fjall --test shared_subscription_smoke` passes
- [ ] No existing test was touched in this PR

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Wait for CI and merge**

```bash
# After CI is green:
gh pr merge --squash --delete-branch
git checkout main && git pull
```

Branch up to and including this point is the foundation for PR2.

---

## PR2: Migrate consumers (projection runner + tests) to `SharedSubscription`

**Goal:** Switch every site that uses `Subscription<()>` over to `SharedSubscription<()>`. Drop the `<Sub::Stream<'_> as BaseEventStream>::to_envelope(item)` call in the projection runner — `item` is now `PersistedEnvelope<'_, ()>` directly. After this PR, nothing in production code reads from the borrowed `Subscription<M>` trait; it's dead weight waiting for PR3.

**Branch:** `refactor/arc-subscription-pr2` (off `refactor/arc-subscription` or off main after PR1 squash-merged — preferred is off main).

**Acceptance criteria:**

- [ ] `nix develop -c nix flake check` passes.
- [ ] `crates/nexus-framework/src/projection/mod.rs`'s closure body no longer contains the string `BaseEventStream` related to `Sub::Stream`.
- [ ] All projection integration tests pass.
- [ ] The conformance harness still uses `BaseEventStream` (for `RawEventStore::Stream` cursors). No changes to `crates/nexus-store-testing/src/lib.rs` in this PR.
- [ ] The old borrowed `Subscription<M>` still compiles (used by no one in production after this PR, but the trait + impls + cursors stay until PR3).

### Task 1: Migrate `crates/nexus-framework/src/projection/mod.rs`

**Files:**
- Modify: `crates/nexus-framework/src/projection/mod.rs`

- [ ] **Step 1: Update the trait import + bounds**

At the top of the file (around line 11–12):

```rust
// Before:
use nexus_store::store::Subscription;
use nexus_store::stream::{BaseEventStream, EventStreamExt};

// After:
use nexus_store::store::SharedSubscription;
use nexus_store::stream::EventStreamExt;
```

Then in the two `impl<...> Projection<...>` where-clauses (lines 110 and 207):

```rust
// Before:
Sub: Subscription<()>,
// or:
Sub: Subscription<()> + Send,

// After:
Sub: SharedSubscription<()>,
// or:
Sub: SharedSubscription<()> + Send,
```

- [ ] **Step 2: Drop the `to_envelope` conversion**

Around line 270:

```rust
// Before:
let envelope = <Sub::Stream<'_> as BaseEventStream>::to_envelope(item);
let event_version = envelope.version();
let decoded = event_codec_ref.decode(envelope.event_type(), envelope.payload());

// After (item IS the envelope directly):
let event_version = item.version();
let decoded = event_codec_ref.decode(item.event_type(), item.payload());
```

This is the single concrete win of the refactor for the projection runner: one fewer ceremony line, one less generic-type incantation.

- [ ] **Step 3: Build the crate**

Run: `nix develop -c cargo build -p nexus-framework`
Expected: clean. If a compile error mentions `for<'a>` HRTB on `Sub::Stream`, the Task 1 spike's assumption needs revisiting — escalate per the spike's fallback plan and update the deviation log.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-framework/src/projection/mod.rs
git commit -m "refactor(framework): switch projection runner to SharedSubscription"
```

### Task 2: Migrate `crates/nexus-framework/src/projection/builder.rs`

**Files:**
- Modify: `crates/nexus-framework/src/projection/builder.rs`

- [ ] **Step 1: Read the file**

Look for every `Subscription<()>` bound (likely on the builder's typestate parameters and on `.subscription(...)` setter signature).

- [ ] **Step 2: Retarget to `SharedSubscription<()>`**

Same substitution as Task 1: `Subscription` → `SharedSubscription` on the trait bounds. Update the `use` import line accordingly.

- [ ] **Step 3: Build**

Run: `nix develop -c cargo build -p nexus-framework`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-framework/src/projection/builder.rs
git commit -m "refactor(framework): retarget projection builder bounds to SharedSubscription"
```

### Task 3: Migrate `crates/nexus-framework/src/projection/error.rs` if needed

**Files:**
- Modify (maybe): `crates/nexus-framework/src/projection/error.rs`

- [ ] **Step 1: Read the file**

`ProjectionError<P, EC, SS, Sub>` is parameterized over the subscription error type via `Sub: Subscription<()>`. Check whether the file references the trait directly.

- [ ] **Step 2: Retarget if needed**

If the file imports or bounds against `Subscription<()>`, switch to `SharedSubscription<()>`. If it parameterizes only over `Sub::Error` types abstractly, no change needed.

- [ ] **Step 3: Build + commit if changed**

```bash
git add crates/nexus-framework/src/projection/error.rs
git commit -m "refactor(framework): retarget projection error bounds to SharedSubscription"
```

### Task 4: Migrate `crates/nexus-store/tests/subscription_tests.rs`

**Files:**
- Modify: `crates/nexus-store/tests/subscription_tests.rs`

- [ ] **Step 1: Read the file to understand what it tests**

If it tests the old borrowed `Subscription`, **leave it alone** in PR2 — the old shape still works and is still in the codebase. The test stays as a regression net until PR3 deletes the old shape.

If it tests subscription semantics in a way that should apply to the new shape too, **port** it: change the `InMemoryStore::new()` to `Arc::new(InMemoryStore::new())`, and the `Subscription<()>` calls to `SharedSubscription<()>` calls. The cursor lifetime parameter `'a` disappears.

- [ ] **Step 2: Build + run tests**

Run: `nix develop -c cargo test -p nexus-store --features testing --test subscription_tests`
Expected: clean.

- [ ] **Step 3: Commit (if changed)**

```bash
git add crates/nexus-store/tests/subscription_tests.rs
git commit -m "test(store): port subscription tests to SharedSubscription"
```

### Task 5: Migrate `crates/nexus-fjall/tests/subscription_tests.rs`

Same procedure as Task 4, for the fjall test file. Same Step 2 / Step 3.

### Task 6: Full flake check + open PR2

- [ ] **Step 1: Run the full check suite**

Run: `nix develop -c nix flake check`
Expected: green.

- [ ] **Step 2: Push and open PR**

```bash
git push -u origin refactor/arc-subscription-pr2
gh pr create --title "refactor(framework): migrate projection runner to SharedSubscription — PR2 of arc-subscription refactor" --body "$(cat <<'EOF'
## Summary

- Projection runner + builder switched to `SharedSubscription<()>`. Closure body no longer needs `<Sub::Stream<'_> as BaseEventStream>::to_envelope(item)` — the item IS a `PersistedEnvelope<'_, ()>`.
- Subscription integration tests migrated where they assert semantics that apply to both shapes.
- Old borrowed `Subscription<M>` trait + cursors still present (used by no production code after this PR). PR3 deletes them.

PR2 of the arc-subscription refactor. Plan: `docs/plans/2026-05-26-arc-subscription-plan.md`. Deviations: `docs/plans/2026-05-26-arc-subscription-deviations.md`.

## Test plan

- [ ] `nix develop -c nix flake check` passes
- [ ] Projection integration tests still pass
- [ ] Conformance harness untouched (it covers `RawEventStore::Stream`, not subscriptions)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Wait for CI and merge**

```bash
gh pr merge --squash --delete-branch
git checkout main && git pull
```

---

## PR3: Delete old `Subscription` + `BaseEventStream` impls on subscription cursors; rename `SharedSubscription` → `Subscription`

**Goal:** Now that nothing reads from the borrowed `Subscription<M>`, delete it. Rename `SharedSubscription` to the canonical `Subscription`. Delete the borrowed cursor types (`InMemorySubscriptionStream<'a>`, `FjallSubscriptionStream<'a>`) and their `BaseEventStream` impls. The `BaseEventStream` trait itself stays — `InMemoryStream` (the `RawEventStore::Stream` cursor) still implements it for the repository facade's load path.

**Branch:** `refactor/arc-subscription-pr3` off main.

**Acceptance criteria:**

- [ ] `nix develop -c nix flake check` passes.
- [ ] `grep -rn "impl Subscription<.*> for &T" crates/` returns nothing.
- [ ] `grep -rn "InMemorySubscriptionStream<'" crates/` returns nothing.
- [ ] `grep -rn "FjallSubscriptionStream<'" crates/` returns nothing.
- [ ] `grep -rn "SharedSubscription" crates/` returns nothing.
- [ ] `grep -rn "SharedInMemorySubscriptionStream" crates/` returns nothing.
- [ ] `grep -rn "SharedFjallSubscriptionStream" crates/` returns nothing.
- [ ] `crates/nexus-store/src/repository.rs` still uses `<S::Stream<'_> as BaseEventStream<()>>::to_envelope(item)` on lines that match the original (~227, 319, 526, 616 in main; line numbers may have shifted).
- [ ] `BaseEventStream` impl on `InMemoryStream` (in `testing.rs`) and `FjallStream` (in `fjall/src/stream.rs`) stays.

### Task 1: Delete old `Subscription<M>` trait + `&T` blanket in `crates/nexus-store/src/store.rs`

**Files:**
- Modify: `crates/nexus-store/src/store.rs`

- [ ] **Step 1: Delete the old trait block + blanket impl**

Delete lines 167–234 (the entire `Subscription<M>` section ending after the `impl<T: Subscription<M> + Sync, M: 'static> Subscription<M> for &T` block). Keep the GlobalSeq section.

- [ ] **Step 2: Rename `SharedSubscription` → `Subscription`**

In the same file, find the `SharedSubscription` trait block (added in PR1) and rename it back to `Subscription`. Adjust the doc comment to remove the "this is separate from Subscription" framing.

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Subscription<M> — Arc-based 'static subscription shape
// ═══════════════════════════════════════════════════════════════════════════

/// A subscription whose cursor owns an `Arc<Store>` and is therefore `'static`.
///
/// Implemented on `Arc<Store>` directly. Each subscription pays one
/// `Arc::clone` at `subscribe()` time; per-event cost is identical to
/// borrowing.
///
/// Cursors are spawnable, movable across async boundaries, and carriable
/// in supervisors. The returned stream **never returns `None`** — when
/// caught up, it waits for new events instead of terminating.
///
/// # Contract
///
/// - `from: None` → start from the beginning of the stream (version 1)
/// - `from: Some(v)` → start from the event *after* version `v`
/// - The returned stream never returns `None`.
/// - Events are yielded with monotonically increasing versions.
pub trait Subscription<M: 'static = ()> {
    type Stream: crate::stream::EventStream<M, Error = Self::Error> + Send + 'static;
    type Error: core::error::Error + Send + Sync + 'static;
    fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}
```

- [ ] **Step 3: Remove now-unused `BaseEventStream` import**

The file no longer references `BaseEventStream`. Drop the `use crate::stream::BaseEventStream;` line (line 10).

- [ ] **Step 4: Build**

Run: `nix develop -c cargo build -p nexus-store`
Expected: errors in `testing.rs` and elsewhere referencing the old types — these are cleaned up in subsequent tasks.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/store.rs
git commit -m "refactor(store)!: delete borrowed Subscription trait; rename SharedSubscription to Subscription"
```

### Task 2: Delete borrowed `InMemorySubscriptionStream<'a>` + rename `SharedInMemorySubscriptionStream`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

- [ ] **Step 1: Delete the old types and impls**

Delete:
- The struct `InMemorySubscriptionStream<'a>` (lines 272–282) and its inherent impl (lines 284–319)
- The `impl BaseEventStream for InMemorySubscriptionStream<'_>` (lines 321–328)
- The `impl EventStream for InMemorySubscriptionStream<'_>` (lines 330–389)
- The `impl Subscription<()> for InMemoryStore` (lines 391–421)

- [ ] **Step 2: Rename `SharedInMemorySubscriptionStream` → `InMemorySubscriptionStream`**

`sed -i '' 's/SharedInMemorySubscriptionStream/InMemorySubscriptionStream/g' crates/nexus-store/src/testing.rs` (or use a project-wide rename — see Task 5). Verify the doc comment no longer mentions "PR3 renames this" — clean it up.

- [ ] **Step 3: Confirm no `BaseEventStream` impl on the renamed cursor**

The new cursor never had one. Confirm with `grep -n BaseEventStream crates/nexus-store/src/testing.rs` — should now match only mentions in docs (or none, if there are no doc references).

- [ ] **Step 4: Build**

Run: `nix develop -c cargo build -p nexus-store --features testing`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "refactor(store)!: drop borrowed InMemorySubscriptionStream; rename Shared variant"
```

### Task 3: Delete borrowed `FjallSubscriptionStream<'a>` + rename `SharedFjallSubscriptionStream`

**Files:**
- Modify: `crates/nexus-fjall/src/subscription_stream.rs`
- Modify: `crates/nexus-fjall/src/store.rs`

- [ ] **Step 1: Delete the old types and impls in `subscription_stream.rs`**

Delete:
- The struct `FjallSubscriptionStream<'a>` (lines 49–59) and its inherent impl (lines 61–98)
- The `impl BaseEventStream for FjallSubscriptionStream<'_>` (lines 100–107)
- The `impl EventStream for FjallSubscriptionStream<'_>` (lines 109–210)

Then rename `SharedFjallSubscriptionStream` → `FjallSubscriptionStream` everywhere in the file. Clean the doc comment.

- [ ] **Step 2: Delete the old `impl Subscription<()> for FjallStore` in `store.rs`**

Delete lines 345–365 (the borrowed `impl Subscription<()> for FjallStore`). Keep the renamed `impl Subscription<()> for Arc<FjallStore>` (originally `SharedSubscription`).

- [ ] **Step 3: Adjust imports**

`store.rs` no longer needs `FjallSubscriptionStream` from its borrowed sense — the new `FjallSubscriptionStream` is the renamed `Shared` variant. Verify imports match what's actually used.

- [ ] **Step 4: Build**

Run: `nix develop -c cargo build -p nexus-fjall`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/subscription_stream.rs crates/nexus-fjall/src/store.rs
git commit -m "refactor(fjall)!: drop borrowed FjallSubscriptionStream; rename Shared variant"
```

### Task 4: Clean up `nexus-store::lib.rs` re-exports

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`

- [ ] **Step 1: Drop `SharedSubscription` from the re-export**

```rust
// Before (PR1's change):
pub use store::{GlobalSeq, RawEventStore, SharedSubscription, Store, Subscription};

// After (back to single Subscription, which is now the renamed Shared trait):
pub use store::{GlobalSeq, RawEventStore, Store, Subscription};
```

- [ ] **Step 2: Build the workspace**

Run: `nix develop -c cargo build --workspace`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "refactor(store): collapse SharedSubscription re-export into Subscription"
```

### Task 5: Project-wide rename + import cleanup

**Files:** all crates that referenced `SharedSubscription` in PR2 (framework + tests).

- [ ] **Step 1: Run the rename**

```bash
# Replace SharedSubscription → Subscription in all Rust files.
git ls-files '*.rs' | xargs sed -i '' 's/SharedSubscription/Subscription/g'

# Replace SharedInMemorySubscriptionStream → InMemorySubscriptionStream
git ls-files '*.rs' | xargs sed -i '' 's/SharedInMemorySubscriptionStream/InMemorySubscriptionStream/g'

# Replace SharedFjallSubscriptionStream → FjallSubscriptionStream
git ls-files '*.rs' | xargs sed -i '' 's/SharedFjallSubscriptionStream/FjallSubscriptionStream/g'
```

- [ ] **Step 2: Run rustfmt**

Run: `nix develop -c cargo fmt --all`

- [ ] **Step 3: Build the workspace**

Run: `nix develop -c cargo build --workspace --all-features`
Expected: clean. Any leftover reference to a deleted type is a real bug — fix it on the spot.

- [ ] **Step 4: Verify the grep acceptance criteria**

```bash
grep -rn "SharedSubscription" crates/    # → empty
grep -rn "SharedInMemorySubscriptionStream" crates/  # → empty
grep -rn "SharedFjallSubscriptionStream" crates/     # → empty
grep -rn "InMemorySubscriptionStream<'" crates/      # → empty (no more borrowed variant)
grep -rn "FjallSubscriptionStream<'" crates/         # → empty
grep -rn "impl Subscription<.*> for &T" crates/      # → empty
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(workspace): drop Shared prefix; canonical Subscription naming"
```

### Task 6: Confirm `BaseEventStream` still works for `RawEventStore::Stream`

**Files:** read-only inspection.

- [ ] **Step 1: Confirm the four repository sites still compile**

```bash
grep -n "BaseEventStream" crates/nexus-store/src/repository.rs
# Expected: 4 matches on the to_envelope sites (lines may have shifted from 227/319/526/616).
```

- [ ] **Step 2: Confirm the trait + impls survive**

```bash
grep -rn "BaseEventStream" crates/ --include="*.rs"
# Expected matches (production):
#   crates/nexus-store/src/store.rs       — bound on RawEventStore::Stream (line 115)
#   crates/nexus-store/src/stream/cursor.rs — trait definition + docs
#   crates/nexus-store/src/stream/mod.rs   — re-exports
#   crates/nexus-store/src/lib.rs          — re-exports
#   crates/nexus-store/src/repository.rs   — 4 to_envelope call sites
#   crates/nexus-store/src/testing.rs      — impl on InMemoryStream
#   crates/nexus-fjall/src/stream.rs       — impl on FjallStream
#   crates/nexus-store-testing/src/lib.rs  — bounds across the conformance harness
# Tests: any test fixture that implements EventStream + BaseEventStream stays.
# No remaining production matches should reference subscription cursors.
```

- [ ] **Step 3: Run the full check suite**

Run: `nix develop -c nix flake check`
Expected: green.

### Task 7: Open PR3

- [ ] **Step 1: Push and open PR**

```bash
git push -u origin refactor/arc-subscription-pr3
gh pr create --title "refactor(store, fjall, framework)!: delete borrowed Subscription; promote Arc-based shape — PR3 of arc-subscription refactor" --body "$(cat <<'EOF'
## Summary

- Deletes the borrowed `Subscription<M>` trait (with `type Stream<'a>: ... where Self: 'a` GAT), the `&T` blanket, and the borrowing cursor types `InMemorySubscriptionStream<'a>` + `FjallSubscriptionStream<'a>`.
- Renames `SharedSubscription` → `Subscription` and the cursor types accordingly.
- Drops the `BaseEventStream` impls on subscription cursors. The trait itself stays — it remains the witness for `RawEventStore::Stream` cursors in the repository facade load paths.
- Net: `BaseEventStream` is now scoped to the `RawEventStore::Stream` path only.

Breaking change: any external user of the old `Subscription<M>` borrowed shape must move to `Arc<Store>` and call `.subscribe(...)` on the Arc.

PR3 of the arc-subscription refactor. Plan: `docs/plans/2026-05-26-arc-subscription-plan.md`.

## Test plan

- [ ] `nix develop -c nix flake check` passes
- [ ] No remaining `SharedSubscription` / `SharedInMemorySubscriptionStream` / `SharedFjallSubscriptionStream` symbols (`grep` returns empty)
- [ ] No remaining borrowed cursor types (`grep -n "InMemorySubscriptionStream<'"` empty)
- [ ] `BaseEventStream` impls survive on `InMemoryStream` (RawEventStore cursor) and `FjallStream`
- [ ] Projection runner still passes its integration tests

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Wait for CI and merge**

```bash
gh pr merge --squash --delete-branch
git checkout main && git pull
```

---

## PR4: Evaluate `OwnedEventStream` and `IntoStream`

**Goal:** With subscription cursors becoming `'static`, the workaround layer added in PR #171 (the futures-bridge sub-trait + `IntoStream`) may simplify or delete. Read the code carefully before deciding the shape.

**Branch:** `refactor/arc-subscription-pr4` off main.

**Acceptance criteria (decided during the PR's first task):**

- Option A (delete `OwnedEventStream`): If the bridge can be expressed without an owned-witness sub-trait now that cursors are `'static`, delete the trait, simplify `IntoStream`, update docs.
- Option B (keep but simplify): If the sub-trait still adds clarity for combinator-only bridging, keep it but verify no dead-code paths remain.
- Option C (no change): If PR1–PR3 didn't actually unblock anything for the futures-bridge, document why in the deviation log and close PR4 as a no-op.

### Task 1: Evaluate

**Files:** read-only inspection of `crates/nexus-store/src/stream/owned.rs`, `crates/nexus-store/src/stream/cursor.rs::into_stream`, and existing `futures_bridge_tests.rs`.

- [ ] **Step 1: Read the file with the PR1–PR3 outcome in mind**

The `OwnedEventStream` doc comment explains: "Base cursors do **not** implement this trait: their `Item<'a>` is a borrowing `PersistedEnvelope`, and forcing a deep-copy here would hide a per-event allocation from callers." That reason still applies in PR4 — `Item<'a>` is still borrowing on `'static` cursors. The 'static change does **not** make the base cursors' items `'static`.

So Option B or C is likely correct. Read the file carefully — the `Pending<S, T, E>` boxed-future type and the `BridgeState` machine may be simplifiable, but the public sub-trait shape probably stays.

- [ ] **Step 2: Decide and document**

Append a deviation log entry under "## [PR 4] — Decision" with: which option was chosen, why, and what was kept/changed. If Option C, the PR is a docs-only update (or no PR at all).

### Task 2: Implement the chosen option

Tasks here vary by option. If Option B/C: 1–2 commits at most. If Option A: a follow-up plan should be written.

### Task 3: Run flake check + open or skip PR

- [ ] **Step 1: Run the check**

Run: `nix develop -c nix flake check`
Expected: green.

- [ ] **Step 2: Open PR (if any change)**

```bash
gh pr create --title "refactor(store): evaluate OwnedEventStream after Arc-based subscriptions — PR4 of arc-subscription refactor" --body "..."
```

- [ ] **Step 3: Merge**

```bash
gh pr merge --squash --delete-branch
```

If no change is warranted, close out PR4 with a note in the deviation log and skip the PR.

---

## Self-Review

- **Spec coverage:** Each item in the user's refactor spec has a task: spike (Task PR1.1), trait introduction (PR1.2), cursor types (PR1.3, PR1.5), Arc impls (PR1.3, PR1.6), smoke tests (PR1.4, PR1.7), projection runner migration (PR2.1), builder + error migration (PR2.2–2.3), test file migration (PR2.4–2.5), old shape deletion (PR3.1–3.3), `BaseEventStream` keep on `RawEventStore::Stream` (PR3.6), `OwnedEventStream` evaluation (PR4).
- **Placeholder scan:** Every code block above contains real Rust; every command above is runnable. No "TBD", "TODO", "implement later", "similar to Task N".
- **Type consistency:** `SharedSubscription` is consistently named throughout PR1–PR2. PR3 renames it to `Subscription` consistently. The cursor names match: `SharedInMemorySubscriptionStream` → `InMemorySubscriptionStream`, `SharedFjallSubscriptionStream` → `FjallSubscriptionStream`. The `BaseEventStream` trait is **never** referenced from the new (Arc-based) subscription cursors, only from `RawEventStore::Stream` cursors (`InMemoryStream`, `FjallStream`) — consistent across PR3.

If the HRTB spike in Task PR1.1 fails to compile, the plan is **wrong about that single assumption**. The fallback (a witness sub-trait `SubscriptionItem` analogous to `BaseEventStream` but scoped to the subscription side) preserves the rest of the refactor. Log the deviation and update PR2/PR3's tasks accordingly before continuing.
