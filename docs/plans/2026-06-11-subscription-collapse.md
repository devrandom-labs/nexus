# Subscription Collapse — Concrete Struct + Hidden Adapter Trait

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the user-facing `Subscription` trait and its `SubscriptionBackend` adapter-facing pair with a single concrete struct `Subscription<S>` rooted at the existing `Store<S>` wrapper, plus one adapter-facing trait `RawSubscription` that is not re-exported at the crate root. Per-stream scope only — `from: Option<Version>`. No `all()` scope, no type-state ceremony for checkpoint/scope pairing — those wait for #176 + a fjall global-seq index decision.

**Architecture:** Users no longer touch `Arc`. They construct `Subscription::new(&store)` where `store: &Store<S>` (the existing `Arc<S>`-backed handle), then call `.subscribe(&id, from)` to obtain a `futures::Stream` cursor. Adapter authors implement `RawSubscription` on their bare store type (the orphan-rule workaround that justified the trait pair is preserved). The trait lives at `nexus_store::subscription::RawSubscription` and is intentionally not re-exported at the crate root — adapter authors who need it import the qualified path; library users never see it. The user-facing trait `Subscription` and the public re-export of `SubscriptionBackend` are deleted.

**Scope:** Single PR. Breaking change (pre-1.0 allowed per project main-merge policy). Conventional commit: `refactor(store, fjall, framework)!: collapse Subscription trait pair to concrete struct (#181)`.

**Tech Stack:** Rust 2024, `bytes::Bytes`, `tokio::sync::Notify` (wake primitive, unchanged), `futures::Stream`, `thiserror`, `nix flake check` for CI.

---

## Out of scope (filed elsewhere)

- **`all()` cross-stream subscription.** Requires a fjall global-seq index — the events partition is keyed `[id_len][id][version]`; the `global` partition holds one counter, no index. Real schema choice (secondary index vs denormalized vs re-key). Gated on #176 landing first.
- **Bounded batch size for cursors.** Issue #176, separate refactor. No API break required.
- **Wake-primitive change from `Notify` to `watch<T>`.** Becomes interesting once `all()` exists (two channels). Not now.
- **Type-state `Subscription<S, Scope, Checkpoint>` from issue #181.** Has nothing to differentiate with one scope; defer until `all()` lands.

---

## File Structure

### New file
- `crates/nexus-store/src/subscription.rs` — `RawSubscription` adapter trait (with the same `(arc: &Arc<Self>, id, from)` shape today's `SubscriptionBackend` has), `Subscription<S>` user-facing struct, internal `sealed::Sealed` super-trait.

### Modified files
- `crates/nexus-store/src/store.rs` — delete `pub trait Subscription`, `pub trait SubscriptionBackend`, and the blanket `impl<T> Subscription for Arc<T>`. Add `pub(crate) fn arc(&self) -> &Arc<S>` accessor for the subscription module.
- `crates/nexus-store/src/lib.rs` — register `pub mod subscription;`; re-export `pub use subscription::Subscription` at crate root; drop `SubscriptionBackend` + old `Subscription` re-exports.
- `crates/nexus-fjall/src/store.rs` — `impl SubscriptionBackend for FjallStore` becomes `impl RawSubscription for FjallStore` + `impl sealed::Sealed for FjallStore` (sealed-trait pattern, see open question below).
- `crates/nexus-store/src/testing.rs` — same rename for `InMemoryStore`.
- `crates/nexus-framework/src/projection/mod.rs` — replace `Sub: Subscription` generic bound with a `Subscription<S>` value field, where `S: RawSubscription`. Call site `sub.subscribe(...)` becomes `self.subscription.subscribe(...)`.
- `crates/nexus-framework/src/projection/builder.rs` — typestate slot `NeedsSub` now takes `Subscription<S>` directly; the `Sub` generic parameter becomes `S: RawSubscription`.
- `crates/nexus-store/tests/subscription_tests.rs` — call-site shape: `Arc::new(InMemoryStore::default()).subscribe(...)` becomes `Subscription::new(&Store::new(InMemoryStore::default())).subscribe(...)`.
- Any other test that bounds on `Subscription` as a trait — rewrite as `RawSubscription` (the type bound) or take a `Subscription<S>` value.
- `CLAUDE.md` (project root) — update the store crate's `Subscription` paragraph in the "Architecture > Store Crate" section.
- `crates/nexus-store/src/lib.rs` rustdoc — module-level description references `Subscription` trait; update to struct.

### Files not changed
- `crates/nexus/` — kernel unchanged.
- `crates/nexus-fjall/src/subscription_stream.rs` — the cursor itself is unchanged. Only the trait it satisfies gets renamed (via the impl in `store.rs`).
- `crates/nexus-store/src/{envelope.rs,wire.rs,value.rs}` — untouched.

---

## Design choices (resolved before writing)

1. **Construction via `Store<S>` wrapper.** `Subscription::new(&store)` pulls the Arc out via a `pub(crate)` accessor. Users never hold raw `Arc`. Cost: using a bare `FjallStore` directly (no `Store` wrapper) no longer works for subscriptions. Examples that bypass the wrapper get small migration cost. Acceptable — `Store::raw` continues to exist as the substrate escape hatch for *reads* (RawEventStore methods) but the subscription path now requires going through the wrapper.

2. **Adapter trait visibility: implementer-facing, not crate-root.** `pub trait RawSubscription` in `nexus_store::subscription`, NOT re-exported at the crate root. Adapter authors import `use nexus_store::subscription::RawSubscription;`. Library users grep `Subscription` and land on the user-facing struct; the adapter primitive never crosses their path.

3. **Trait name: `RawSubscription`.** Mirrors `RawEventStore` — the established `Raw*` pattern in the crate signals "implementer-facing adapter primitive."

4. **`from` parameter: stay `Option<Version>`.** No builder chain, no `.checkpoint()` method. When `all()` lands, the API can break (pre-1.0). The current PR stays small.

5. **Wake primitive: `Notify` unchanged.** No motion until `all()`.

---

## Open question to resolve in review

**Cross-crate sealing strictness.** The user requested a "sealed adapter trait." True cross-crate sealing (where only `nexus-store` itself can implement `RawSubscription`) conflicts with the fact that `nexus-fjall` and `nexus-store::testing::InMemoryStore` are separate adapters — fjall is in its own crate. The practical interpretations:

- **A (chosen for this plan)**: *Soft sealing via visibility.* `pub mod sealed { pub trait Sealed {} }` is `pub` (so external adapter crates can `impl sealed::Sealed for FjallStore`), but the `subscription` module itself does not re-export `Sealed` or `RawSubscription` at the crate root, and the rustdoc clearly marks them as adapter-only. Third-party adapters CAN reach in and implement — there is no hard barrier. The signal is documentary, not structural.

- **B**: *Hard sealing, locked to first-party adapters.* Pull `InMemoryStore` and fjall back into `nexus-store` (collapses the crate boundary) — not desirable.

- **C**: *Hard sealing via a versioned token trait.* Each adapter passes a `&'static InternalToken` from `nexus-store` to register itself. Overengineered for the current value proposition.

Going with A. If you want B or C, the plan needs revision before any code is touched. Otherwise no further action needed.

---

## Task checklist

Setup
- [ ] **0.1** Confirm branch `refactor/subscription-collapse` is checked out (already done at plan-write time).

Module addition
- [ ] **1.1** Create `crates/nexus-store/src/subscription.rs`:
    - `mod sealed { pub trait Sealed {} }` (private module, `pub` trait → standard sealed-trait shape).
    - `pub trait RawSubscription: sealed::Sealed + Send + Sync + 'static` with:
        - `type Stream: EventStream<Error = Self::Error> + 'static`
        - `type Error: core::error::Error + Send + Sync + 'static`
        - `fn subscribe(arc: &Arc<Self>, id: &impl Id, from: Option<Version>) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;`
    - `pub struct Subscription<S> { store: Arc<S> }` (single field, `S: RawSubscription`).
    - `impl<S: RawSubscription> Subscription<S>`:
        - `pub fn new(store: &Store<S>) -> Self { Self { store: Arc::clone(store.arc()) } }`
        - `pub async fn subscribe(&self, id: &impl Id, from: Option<Version>) -> Result<S::Stream, S::Error> { S::subscribe(&self.store, id, from).await }`
    - Rustdoc: explain that `RawSubscription` is adapter-facing and the user-facing entry is `Subscription::new(&store)`.

Store wrapper exposure
- [ ] **2.1** In `crates/nexus-store/src/store.rs`, add `pub(crate) fn arc(&self) -> &Arc<S> { &self.inner }` on `Store<S>`. This is `pub(crate)` so the subscription module can reach it but it doesn't leak `Arc` to users.

Trait deletion
- [ ] **3.1** Delete `pub trait Subscription` (current store.rs:197–215).
- [ ] **3.2** Delete `pub trait SubscriptionBackend` (current store.rs:238–255).
- [ ] **3.3** Delete blanket `impl<T> Subscription for Arc<T> where T: SubscriptionBackend` (current store.rs:259–273).
- [ ] **3.4** Update `Store::raw` doc comment (currently references `Subscription` trait usage patterns — adjust).

Re-exports
- [ ] **4.1** In `crates/nexus-store/src/lib.rs`:
    - Add `pub mod subscription;`.
    - Change `pub use store::{GlobalSeq, RawEventStore, Store, Subscription, SubscriptionBackend};` to `pub use store::{GlobalSeq, RawEventStore, Store};` and `pub use subscription::Subscription;`.
    - Update the module-level docstring's mention of `Subscription` (currently describes a trait).

Adapter migrations
- [ ] **5.1** In `crates/nexus-fjall/src/store.rs`:
    - Change `use nexus_store::store::{RawEventStore, SubscriptionBackend};` to `use nexus_store::store::RawEventStore; use nexus_store::subscription::{RawSubscription, sealed::Sealed};` (or whatever names land).
    - Change `impl SubscriptionBackend for FjallStore` to `impl RawSubscription for FjallStore`.
    - Add `impl Sealed for FjallStore {}`.
- [ ] **5.2** In `crates/nexus-store/src/testing.rs`:
    - Same shape — `RawSubscription` impl + `Sealed` impl for `InMemoryStore`.
    - The `Sealed` impl here lives within `nexus-store`, so it's a direct `impl crate::subscription::sealed::Sealed for InMemoryStore {}`.

Consumer migrations
- [ ] **6.1** In `crates/nexus-framework/src/projection/mod.rs`:
    - Change `Sub: Subscription` generic bound to `S: RawSubscription` (rename the type parameter for clarity).
    - Replace the `subscription: Sub` field type with `subscription: nexus_store::Subscription<S>`.
    - Call site `sub.subscribe(&id, status.checkpoint())` becomes `self.subscription.subscribe(&id, status.checkpoint()).await`.
    - Error mapping shape unchanged — still routes through `ProjectionError::Subscription(S::Error)`.
- [ ] **6.2** In `crates/nexus-framework/src/projection/builder.rs`:
    - `NeedsSub` typestate slot now holds `Subscription<S>` (concrete).
    - The generic parameter renames from `Sub` to `S`.

Tests
- [ ] **7.1** `crates/nexus-store/tests/subscription_tests.rs` — update construction shape. Existing assertions stay; only the API surface changes.
- [ ] **7.2** Grep workspace-wide for `Subscription` trait bounds in tests; convert.
- [ ] **7.3** Confirm no test changes its assertions (this is a shape-only refactor). If any test needs a semantic change, that's a bug — investigate, don't paper over.

Docs
- [ ] **8.1** `CLAUDE.md` — store crate section, the bullet on `Subscription`. Rewrite to "concrete struct, built via `Subscription::new(&store)`; the adapter primitive `RawSubscription` lives in `subscription` and is not re-exported at the crate root."
- [ ] **8.2** `crates/nexus-store/src/lib.rs` rustdoc — module-level paragraph references `Subscription` as a trait. Update.

Verification gates
- [ ] **9.1** `nix develop -c cargo fmt --all`.
- [ ] **9.2** `nix flake check` — MUST pass before commit.
- [ ] **9.3** Squash-commit on the feature branch with message `refactor(store, fjall, framework)!: collapse Subscription trait pair to concrete struct (#181)`.
- [ ] **9.4** Open PR; expect Nix Flake Check + signed commits gate per project main-merge policy.

---

## Test strategy

The 4 mandatory cross-cutting categories (per CLAUDE.md rule #7) are **already covered** by the existing subscription test suite. This refactor is API-shape only — no behavior change is intended.

| Category | Existing coverage | Action |
|---|---|---|
| Sequence/protocol | `subscription_tests.rs` — subscribe → yield → catch up → wait → yield | Update call-site shape, assertions unchanged |
| Lifecycle | `fjall` open/close/reopen tests | No subscription-specific lifecycle change |
| Defensive boundary | Version monotonicity, from > current | Same |
| Linearizability/isolation | Concurrent writer + subscriber | Same |

Signal: if any existing test needs a *semantic* change (not just syntactic), that's a sign behavior leaked. Investigate before merging.

---

## Risk + rollback

- **Risk:** projection runner's typestate builder change has the widest blast radius. Mitigation: take the consumer migration last; let the trait deletion + adapter renames stabilize first.
- **Rollback:** entire PR reverts cleanly (single squash commit). Old trait pair can be reintroduced if needed — no on-disk format change, no migration.
