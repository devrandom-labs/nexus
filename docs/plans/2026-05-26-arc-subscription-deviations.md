# Deviation Log — Arc-based Subscription Refactor

Companion to `docs/plans/2026-05-26-arc-subscription-plan.md`.

Every implementation step that diverges from the plan, makes an
assumption not specified there, skips planned work, adds unplanned
work, or renames things appends an entry below using the schema in
the "Schema" section.

This file is the audit trail. The plan is intent. If a deviation
reveals the plan was wrong, update the plan **and** log the deviation
here.

---

## Schema (copy this when adding entries)

```
## [PR N | Task M] — [short title]
- **Type**: deviation | assumption | skipped | added | renamed | open-point-resolved
- **Plan said**: <quote or paraphrase from the plan>
- **Actually did**: <what happened in the implementation>
- **Reason**: <why — design discovery, blocker, scope decision, missing context>
- **Impact**: <how this affects later PRs or the target design>
- **Date / PR link**: <YYYY-MM-DD, and link to the PR if open>
```

---

## Entries

## [PR 1 | Task 3] — `Arc<Notify>` cloned before `notified()` to avoid borrow conflict
- **Type**: deviation
- **Plan said**: PR1 Task 3's verbatim cursor body uses `let notified = self.store.notify.notified();` — "Body byte-for-byte identical to the borrowed variant; only `store: Arc<...>` vs `store: &'a ...` differs; `Arc::Deref` is transparent."
- **Actually did**: `let notify = Arc::clone(&self.store.notify); let notified = notify.notified();` — clone the inner `Arc<Notify>` so the resulting `Notified<'_>` future doesn't borrow from `self`. The subsequent `self.refill(from).await` is a `&mut self` call.
- **Reason**: The "byte-for-byte" claim was wrong. The borrowed variant's `store: &'a InMemoryStore` is **external** to `self` — accessing `self.store.notify` does not borrow `self`. With `store: Arc<InMemoryStore>` living inside `self`, the borrow of `self.store.notify` is a borrow of `self`, which conflicts with the subsequent `&mut self` refill call (E0502).
- **Impact**: Functional behavior unchanged — `Notified<'_>` is still registered before the read, preserving the race-free ordering. One extra atomic ref-count bump per empty-buffer loop iteration (negligible). The same pattern applies to PR1 Task 5 (fjall cursor — same `Arc<FjallStore>::notify` access); the verbatim body in that task needs the same correction. Plan body code blocks are stale; the executor must apply this pattern to the fjall cursor too.
- **Date / PR link**: 2026-05-26, branch `refactor/arc-subscription`, commit `b771d7a`

## [PR 1 | Task 3] — `Self::clone(self)` instead of `Arc::clone(self)` in the `subscribe` impl
- **Type**: deviation / lint-driven
- **Plan said**: `store: Arc::clone(self)` inside the `SharedInMemorySubscriptionStream` construction in the `subscribe` impl.
- **Actually did**: `store: Self::clone(self)` — same semantics (in this impl block `Self == Arc<InMemoryStore>`), satisfies `clippy::use_self` (nursery, workspace-denied).
- **Reason**: `Arc::clone(self)` triggers `clippy::use_self` because we're inside `impl ... for Arc<InMemoryStore>` and `Self` is the more local name.
- **Impact**: None — `Self::clone` and `Arc::clone` resolve to the same function pointer here. The same pattern applies to the fjall task (PR1 Task 6).
- **Date / PR link**: 2026-05-26, branch `refactor/arc-subscription`, commit `b771d7a`

## [PR 1 | Task 5] — `FjallStore::notify` is `Notify`, not `Arc<Notify>` — clone the store Arc instead
- **Type**: deviation / API-asymmetry-discovered
- **Plan said**: `let notify = Arc::clone(&self.store.notify); let notified = notify.notified();` — clone the inner `Arc<Notify>` to detach the `Notified` future from `&self`. This pattern came from the in-memory cursor where `InMemoryStore::notify: Arc<Notify>`.
- **Actually did**: `let store = Arc::clone(&self.store); let notified = store.notify.notified();` — clone the outer `Arc<FjallStore>` and read `notify` from the clone. The `Notified` future borrows from the cloned `FjallStore` (specifically its `notify` field), not from `self`.
- **Reason**: `FjallStore::notify` is `Notify` (bare), while `InMemoryStore::notify` is `Arc<Notify>`. The two stores have asymmetric inner ownership of the notifier — `FjallStore` keeps the `Notify` inline, `InMemoryStore` wraps it in an `Arc` for the borrowed subscription cursor's needs. Cloning the outer `Arc<FjallStore>` and reading `notify` through the clone achieves identical race-free semantics: `Notified` registers before refill, and the local `store` clone lives through the iteration so the borrow is valid until `notified.await`.
- **Impact**: One extra atomic ref-count bump per empty-buffer loop iteration on `Arc<FjallStore>` instead of on `Arc<Notify>`. Negligible. The in-memory cursor pattern (`Arc::clone(&self.store.notify)`) does NOT apply to the fjall side — they require different approaches. PR1.3's deviation note that "the same pattern applies to PR1 Task 5" was wrong; this is a different pattern with the same end goal.
- **Date / PR link**: 2026-05-26, branch `refactor/arc-subscription`, commit `07c0ad2`

## [PR 1 | Task 6] — Orphan rule blocks `impl SharedSubscription<()> for Arc<FjallStore>` in adapter crate; pivot to delegate-trait blanket
- **Type**: deviation / architectural-pivot
- **Plan said**: `impl SharedSubscription<()> for Arc<FjallStore>` in `nexus-fjall/src/store.rs`, mirroring `impl SharedSubscription<()> for Arc<InMemoryStore>` in `nexus-store/src/testing.rs`.
- **Actually did**: Stopped on E0117. `SharedSubscription` is defined in `nexus-store`, `Arc` is from `std`, and `FjallStore` sits at a *covered* position inside `Arc<FjallStore>` — not the *local-type-at-uncovered-position* the orphan rule requires. The in-memory variant works only because its `impl` lives in the same crate as the trait. After human decision (Option B), pivoted to a delegate-trait blanket: `nexus-store` defines a second trait `SharedSubscriptionBackend<M>` and a blanket `impl<T, M> SharedSubscription<M> for Arc<T> where T: SharedSubscriptionBackend<M>`. Adapters implement `SharedSubscriptionBackend` on the **bare** store type (`InMemoryStore`, `FjallStore`) — local type, no covered position, coherence happy. User-facing API is unchanged: `store.subscribe(&id, None)` still works for `store: Arc<InMemoryStore>` and `store: Arc<FjallStore>` via the blanket.
- **Reason**: The original plan was symmetric in spirit but Rust's coherence rules forbade implementing `SharedSubscription` for `Arc<FjallStore>` in `nexus-fjall`. Newtype wrapper (Option A) was rejected because it leaks orphan-rule plumbing into the user-facing API forever. `self: Arc<Self>` receiver (Option D) was rejected because it consumes the Arc per call, forcing `Arc::clone(&store).subscribe(...)` at every call site.
- **Impact**: Adds one trait to `nexus-store`'s public API (`SharedSubscriptionBackend<M>` — kept thin, doc-comment explains "you implement this; users get `SharedSubscription` via the blanket"). The in-memory impl moves from `impl SharedSubscription<()> for Arc<InMemoryStore>` to `impl SharedSubscriptionBackend<()> for InMemoryStore` — same logic, different shape, smoke test still passes unchanged because the user-facing trait is reached via the blanket. PR3's rename (`SharedSubscription` → `Subscription`) extends to: rename `SharedSubscriptionBackend` → `SubscriptionBackend` at the same time, or merge the two into one trait if a cleaner design emerges. The cursor types and the per-event cost story are unaffected.
- **Date / PR link**: 2026-05-26, branch `refactor/arc-subscription`, decision made before commit (E0117 blocked the attempt)

## [PR 1 | Task 5] — `#[allow(dead_code)]` on the cursor struct + inherent impl
- **Type**: added / transitional
- **Plan said**: No `#[allow(dead_code)]` mentioned.
- **Actually did**: Added `#[allow(dead_code, reason = "constructor wired in PR1.6")]` (or equivalent) on `SharedFjallSubscriptionStream` struct and its inherent `impl` block.
- **Reason**: The cursor's `pub(crate) const fn new(...)` constructor and its inherent methods (`next_read_version`, `refill`) are only called from `impl SharedSubscription<()> for Arc<FjallStore>`, which lands in PR1.6. Workspace clippy treats `dead_code` as an error (`-D warnings`), so the build fails without the allow.
- **Impact**: Transitional. PR1.6 (next task) wires the constructor + impl; the `#[allow(dead_code)]` attributes should be removed in that same commit (or a follow-up) once the call sites exist.
- **Date / PR link**: 2026-05-26, branch `refactor/arc-subscription`, commit `07c0ad2`
