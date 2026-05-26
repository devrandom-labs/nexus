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
