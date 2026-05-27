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

## [PR 2 | Task 2] — `builder.rs` retarget skipped: no `Subscription<()>` bound exists there
- **Type**: skipped
- **Plan said**: Task 2 — "Look for every `Subscription<()>` bound (likely on the builder's typestate parameters and on `.subscription(...)` setter signature)." then "Same substitution as Task 1: `Subscription` → `SharedSubscription` on the trait bounds. Update the `use` import line accordingly."
- **Actually did**: Read `crates/nexus-framework/src/projection/builder.rs` end-to-end. The builder has zero `Subscription<()>` bounds and zero references to the `Subscription` trait. Its `.subscription(sub: NewSub)` setter is fully generic with no bound. The terminal `.build()` impl requires only `Sub: Send + Sync`, `SS: Send + Sync`, `P: Send + Sync`, `EC: Send + Sync`. The trait narrowing happens in `mod.rs`'s `Configured` and `Ready<P::State>` impl blocks, which are the only places that demand `SharedSubscription<()>`. No change to `builder.rs`.
- **Reason**: The plan over-anticipated where the bound lived. The actual design keeps the builder pure typestate-construction scaffolding (the `Needs*` markers are `!Send`, so `.build()` is the only gate, and it gates on `Send + Sync` not on the subscription trait). This is consistent with the CLAUDE.md note on `builder.rs`: "Kept separate from `repository.rs` because it's pure typestate construction scaffolding."
- **Impact**: None. Plan Task 2 is a no-op for the existing code. If a future builder API exposes subscription-specific helpers (e.g. `.subscription_resume_from(version)`), the bound would land then. PR3's rename (`SharedSubscription` → `Subscription`) does not need to touch this file either.
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr2`, commit `e979f58`

## [PR 2 | Task 3] — `error.rs` doc-comment retarget (documentation-only change)
- **Type**: deviation
- **Plan said**: Task 3 — "Check whether the file references the trait directly… If the file imports or bounds against `Subscription<()>`, switch to `SharedSubscription<()>`. If it parameterizes only over `Sub::Error` types abstractly, no change needed."
- **Actually did**: The file's *generic parameters* are abstract (`<P, EC, SS, Sub>` with no trait bound on `Sub` — the `From<Sub>` impl is shape-driven, not bound-driven), so per the plan's "no change needed" branch nothing structural was touched. BUT the doc-comment above the `From<Sub> for ProjectionError` impl named the now-irrelevant trait twice: `Subscription::Stream<'a>::Error == Subscription::Error` and `[\`nexus_store::store::Subscription\`]`. Retargeted both to the new trait the runner actually uses: `<Self::Stream as EventStream<()>>::Error == Self::Error` and `[\`nexus_store::store::SharedSubscription\`]`. The new wording reflects that `SharedSubscription::Stream` is concrete (non-GAT) — there's no `'a` lifetime on `Stream` to thread through, only on `EventStream::Item<'a>`, which doesn't appear in the error-type identity.
- **Reason**: Doc accuracy. The doc-comment's role is to tell the next reader *why* the `From` impl exists and *what trait identity* makes it work. Leaving the old wording would point at the dead-weight borrowed trait that PR3 deletes, and the `Stream<'a>::Error == Error` identity does not even exist on the new trait (its `Stream` is not GAT'd).
- **Impact**: None on behavior — pure documentation. The `From<Sub>` impl is unbounded by either trait, so it covers both subscription shapes during the PR2→PR3 transition. PR3's rename will revisit this doc comment (drop the `Shared` prefix); the structural form `<Self::Stream as EventStream<()>>::Error == Self::Error` stays correct under the rename.
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr2`, commit `e979f58`

## [PR 2 | Tasks 4 + 5] — Subscription integration tests in `nexus-store` and `nexus-fjall` left untouched (regression net until PR3)
- **Type**: skipped
- **Plan said**: Task 4 — "If it tests the old borrowed `Subscription`, **leave it alone** in PR2 — the old shape still works and is still in the codebase. The test stays as a regression net until PR3 deletes the old shape. If it tests subscription semantics in a way that should apply to the new shape too, **port** it…" Same for Task 5 (fjall).
- **Actually did**: Per controller decision (no human escalation needed; the plan explicitly authorizes this branch), both `crates/nexus-store/tests/subscription_tests.rs` and `crates/nexus-fjall/tests/subscription_tests.rs` were left unmodified. They exercise the borrowed `Subscription<M>` trait, which is still alive in the codebase. The `shared_subscription_smoke.rs` files added in PR1 cover the new `SharedSubscription<M>` shape sufficiently for PR2's scope. PR3 will delete both the borrowed trait and these tests together (and may port the test bodies onto the renamed trait at that time).
- **Reason**: Defense in depth during the transition. The borrowed shape is dead weight only in production code after PR2; the trait + cursors + tests stay until PR3 to keep the safety net under both shapes while consumers migrate. Porting now would orphan one of the two shapes and reduce coverage on the surviving one without any compensating gain.
- **Impact**: Zero behavioral. PR3 owns the deletion of: borrowed `Subscription<M>` trait, borrowed cursor types, `BaseEventStream` if no other consumer needs it, and both `subscription_tests.rs` files (with optional port to `shared_subscription_smoke.rs`-style coverage).
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr2`, commit `e979f58`

## [PR 2 | Task 1, Step 2] — Closure body needs HRTB type-equality on `Sub::Stream::Item<'a>` to call `PersistedEnvelope` methods directly
- **Type**: deviation / API-discovery
- **Plan said**: Step 2 — "After (item IS the envelope directly): `let event_version = item.version(); let decoded = event_codec_ref.decode(item.event_type(), item.payload());` This is the single concrete win of the refactor for the projection runner: one fewer ceremony line, one less generic-type incantation." Then Step 3: "If a compile error mentions `for<'a>` HRTB on `Sub::Stream`, the Task 1 spike's assumption needs revisiting — escalate per the spike's fallback plan and update the deviation log."
- **Actually did**: The simple substitution produced E0599 (`no method named version` / `event_type` / `payload` on the associated type `<<Sub as SharedSubscription>::Stream as EventStream>::Item<'_>`). The trait bound `Sub: SharedSubscription<()>` only guarantees `Stream: EventStream<()> + Send + 'static`; it does *not* constrain `EventStream::Item<'a>` (combinator streams override it to closure output, so `Item<'a>` is genuinely abstract). Added one HRTB bound on the `Ready<P::State>::run` impl block: `Sub::Stream: for<'a> EventStream<(), Item<'a> = PersistedEnvelope<'a, ()>>`. Also added `use nexus_store::envelope::PersistedEnvelope;` and added `EventStream` to the existing `nexus_store::stream` import. With the bound in place, `item.version() / item.event_type() / item.payload()` resolve as direct `PersistedEnvelope` method calls — the plan's "concrete win" (one fewer ceremony line, no `BaseEventStream::to_envelope`) is preserved.
- **Reason**: The plan's spike assumption — that `Sub::Stream` being concrete-and-`'static` would let the compiler infer `Item<'a> = PersistedEnvelope<'a, ()>` automatically — was incorrect. `Item<'a>` is an unconstrained associated type on `EventStream`, so the compiler has no way to know what it is at the consumer call site without an explicit bound. The HRTB form `for<'a> EventStream<(), Item<'a> = PersistedEnvelope<'a, ()>>` IS the form the plan's spike was worried about, but in this case it resolves cleanly because (a) `Sub::Stream: 'static` (from `SharedSubscription`'s `type Stream: ... + 'static`) makes `for<'a> Sub::Stream: 'a` trivially true and removes the `where Self: 'a` HRTB nesting problem the plan flagged. The earlier `Sub::Stream<'_>` failure mode (where `Stream` itself was a GAT) does not apply here — `SharedSubscription::Stream` is non-GAT by design.
- **Impact**: Adds one trait bound and two import lines to `mod.rs`. The bound narrows the runner's `run()` impl to subscription cursors whose items are `PersistedEnvelope` (i.e., base cursors, not combinator outputs). This is the *correct* semantic anyway — the runner decodes envelopes, so it must receive envelopes. If a future user wants to chain a combinator before the runner, they would need to feed envelope-shaped output back in. PR3's rename (`SharedSubscription` → `Subscription`) leaves this bound's structural form unchanged; only the trait name in the `Sub:` bound flips. The plan's "concrete win" framing remains accurate at the call-site level (no `to_envelope` ceremony); the cost was paid at the where-clause level instead.
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr2`, commit `e979f58`

## [PR 2 | Unplanned] — Integration test migration: `Arc<InMemoryStore>` for `SharedSubscription` blanket
- **Type**: added
- **Plan said**: Plan PR2 focused on `nexus-framework/src/projection/{mod,builder,error}.rs` and the `subscription_tests.rs` files in store/fjall. It did not call out `crates/nexus-framework/tests/projection_tests.rs` as needing changes.
- **Actually did**: After retargeting `mod.rs`'s impl blocks to `SharedSubscription<()>`, 28 call sites in `projection_tests.rs` failed to compile with `&InMemoryStore: SharedSubscription` not satisfied. The blanket `impl<T, M> SharedSubscription<M> for Arc<T> where T: SharedSubscriptionBackend<M>` requires `Arc<InMemoryStore>`, not `&InMemoryStore`. Mechanical migration: added `use std::sync::Arc;`; converted every `let store = InMemoryStore::new();` → `let store = Arc::new(InMemoryStore::new());` (replace_all, ~20 sites); converted every `.subscription(&store)` → `.subscription(Arc::clone(&store))` (replace_all, 28 sites). `append_events(store: &InMemoryStore, …)` calls continue to work unchanged because `Arc<T>` deref-coerces to `&T`. All 32 framework tests (12 unit + 20 integration) pass after the migration.
- **Reason**: Plan omission. The plan's PR2 task list enumerated the source files plus the two adapter-level subscription test files but missed the consumer-side integration tests in the same crate. The compile-time discovery is the direct mechanical consequence of switching the trait bound — `SharedSubscription` only reaches `&InMemoryStore` via the `Arc<T>` blanket, never directly.
- **Impact**: One test file diff: import + 20 store bindings + 28 subscription wiring sites. No semantic change to what is tested; only the construction shape changed. Future tests in this file should follow the `Arc::new(InMemoryStore::new())` + `Arc::clone(&store)` pattern. PR3's rename does not need to revisit this file (the call shape stays the same).
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr2`, commit `e979f58`

## [PR 3 | Controller decision] — Single atomic commit instead of per-task commits
- **Type**: deviation
- **Plan said**: Tasks 1–5 each end with their own `git add … && git commit -m "…"` step. Plan implicitly assumed per-task commits could be staged independently.
- **Actually did**: One coherent `refactor(store, fjall, framework)!: …` commit covering all source deletions + renames + test ports. Intermediate states are not committed.
- **Reason**: `.githooks/pre-commit` runs `nix flake check` on any non-docs commit. After Task 1 deletes the borrowed `Subscription<M>` trait, every dependent crate (`nexus-store::testing`, `nexus-fjall`, `nexus-framework`) fails to compile until Task 5's project-wide rename completes. So commits at the end of Tasks 1, 2, 3, or 4 would all be blocked by the hook. The rules forbid `--no-verify`. The only viable shape is one commit at the first green point (which is after Task 5).
- **Impact**: The plan's per-task commit messages are inputs to the final commit's body (as a delta summary), but only one SHA exists. Future plans for deletion/rename refactors that touch multiple crates should default to a single-commit shape unless the team adds a "WIP commit" workflow that bypasses the hook for known-broken intermediates. The squash-merge ruleset on `main` collapses everything to one SHA anyway, so per-task commits would only have helped local bisection — which is moot when intermediate states do not even compile.
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr3`

## [PR 3 | Task 1 — Controller decision on `SharedSubscriptionBackend`] — Rename to `SubscriptionBackend`, do not collapse
- **Type**: open-point-resolved
- **Plan said**: "Decide whether to rename `SharedSubscriptionBackend` to `SubscriptionBackend` or collapse into a single trait if a cleaner design emerges — escalate to me if you go for the collapse." (User briefing, echoing PR1 Task 6 deviation.)
- **Actually did**: Rename only. `SharedSubscriptionBackend` → `SubscriptionBackend`. The trait pair (`Subscription` user-facing + `SubscriptionBackend` adapter-facing + blanket `impl<T> Subscription<M> for Arc<T> where T: SubscriptionBackend<M>`) stays as-is, with the `Shared` prefix stripped throughout.
- **Reason**: The trait pair exists because of Rust's orphan rule (PR1 Task 6 deviation, E0117). Collapsing requires either (a) re-trying `self: Arc<Self>` receiver (already rejected — consumes the Arc per call), (b) a new `self: &Arc<Self>` arbitrary-self-type receiver shape (would change the trait signature and re-introduce per-call boilerplate at every consumer site), or (c) merging two responsibilities into one trait (loses the adapter/user separation that makes the blanket work). None of those is "cleaner" — each trades coherence ergonomics for a different ergonomic tax. Rename keeps the proven shape and removes only the temporary `Shared` prefix that PR1 introduced as scaffolding.
- **Impact**: `SubscriptionBackend<M>` joins `Subscription<M>` in `nexus-store::store`. The doc comment that previously framed the trait as "PR3 renames or merges this" is now updated to its permanent form: "Adapter-facing primitive for the `Subscription` blanket impl; exists because of E0117." The "PR3 evaluates collapse" open question is closed.
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr3`

## [PR 3 | Controller decision on tests] — Port `subscription_tests.rs` to new shape; delete `shared_subscription_smoke.rs`
- **Type**: deviation
- **Plan said**: PR2's deviation log left the choice open: "PR3 owns the deletion of: borrowed `Subscription<M>` trait, borrowed cursor types, `BaseEventStream` if no other consumer needs it, and both `subscription_tests.rs` files (with optional port to `shared_subscription_smoke.rs`-style coverage)." The user briefing repeated the choice: "Decide based on whether the assertions still cover semantics worth keeping."
- **Actually did**: Port the 9 tests in `crates/nexus-store/tests/subscription_tests.rs` and the 9 tests in `crates/nexus-fjall/tests/subscription_tests.rs` onto the renamed `Subscription` trait (mechanical: `&store` → `Arc<store>` + `Arc::clone(&store)` at call sites, `use nexus_store::store::Subscription` import is unchanged after the rename). Fold the static-ness compile-time assertion (`fn assert_static<T: 'static>(_: &T) {}` applied to the cursor) from `shared_subscription_smoke.rs` into the ported file. Delete both `shared_subscription_smoke.rs` files — their coverage is now a strict subset of the ported `subscription_tests.rs`.
- **Reason**: The borrowed-trait `subscription_tests.rs` files carry 9 tests each — sequence/protocol, lifecycle, defensive boundary coverage per the project's "4 cross-cutting categories first" rule. The smoke files carry 2 tests each — sanity-only. Deleting the substantive coverage to keep the sanity coverage would actively reduce the regression net under the renamed trait. Porting is mechanical (the trait surface is identical: `subscribe(&self, &id, Option<Version>)` on a value the smoke tests already construct as `Arc<Store>`).
- **Impact**: After PR3, each adapter crate has one canonical `subscription_tests.rs` covering both the old behavioral surface AND the new static-ness guarantee. No `shared_*` files remain anywhere. Test count: 9 + 1 static-ness = 10 per adapter crate. Coverage delta vs. PR2: zero behavioral, net-positive ergonomics (one file per adapter instead of two).
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr3`

## [PR 3 | Verification — pre-existing `--all-targets` clippy issues in `nexus-store` test files]
- **Type**: open-point-resolved
- **Plan said**: PR3 user briefing's verification step 4: "Per-crate clippy (the workspace flake check only runs `--lib`, so run this to catch what flake check misses): `cargo clippy -p nexus-store --all-targets -- -D warnings`."
- **Actually did**: `cargo clippy -p nexus-store --all-targets -- -D warnings` fails with 7 errors — but all 7 are pre-existing on `main` (verified via `git stash` + re-run on clean tree), confined to `crates/nexus-store/tests/futures_bridge_tests.rs` and `crates/nexus-store/tests/stream_tests.rs`. Specifically `clippy::missing_const_for_fn` on test helpers (e.g. `fn is_send<T: Send>() {}`) and `clippy::significant_drop_tightening` on test bodies. None of these files are touched by PR3. `cargo clippy -p nexus-store --lib -- -D warnings`, `cargo clippy -p nexus-store --test subscription_tests -- -D warnings`, `cargo clippy -p nexus-fjall --all-targets`, and `cargo clippy -p nexus-framework --all-targets` are all clean. The official CI gate (`nix flake check`, which uses `--lib` for the clippy derivation) passes. Filed nothing in this PR; logged here for visibility so a future PR can clean these up under "test infra hygiene" without scope-creeping PR3.
- **Reason**: The pre-existing failures originated in PR #171 (futures-bridge) and the `stream_tests.rs` set — neither touched by the arc-subscription refactor. Fixing them inside PR3 would expand the scope from "delete borrowed subscription + rename" to "delete borrowed subscription + rename + unrelated test cleanup", violating the squash-commit single-concern norm. They surface only when running `--all-targets`, which the project CI does not currently do.
- **Impact**: PR3 ships green under the actual CI gate (`nix flake check`). Future hygiene PR can add `--all-targets` to the flake's clippy check after fixing the existing test-only lints (or `#[allow(clippy::missing_const_for_fn, reason = "test helper")]` on the affected fns).
- **Date / PR link**: 2026-05-27, branch `refactor/arc-subscription-pr3`
