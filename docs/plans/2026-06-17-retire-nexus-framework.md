# Retire `nexus-framework` — Projection is Primitives, the Loop is the Consumer's

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the `nexus-framework` crate *and* its projection FSM. nexus keeps only the pure event-sourcing primitives it already ships (`Projector`, `PersistTrigger`, `Subscription`, `SnapshotStore`); the event loop that wires them is demonstrated once in `examples/projection-tokio` and otherwise owned by each consumer's runtime.

**Architecture — the ownership line this enforces:**

| Owns | nexus | Agency (`devrandom-labs/agency`) |
|---|---|---|
| Pure ES primitives: `Projector` (how to fold), `PersistTrigger` (when to persist), `Subscription` (cursor), `SnapshotStore` (atomic `(state, position)` commit) | ✅ single source of truth | consumes |
| The loop / actor / supervision / cursor lifecycle / passivation | ❌ never — that would make nexus a runtime | ✅ owns (P1.5-M1 kameo-fork over Zenoh; #138 AID-keyed cursor passivation/resume) |

**Why the FSM is deleted, not moved.** The `ProjectionStatus` (Idle/Pending/Committed) + `apply_event` machine in `nexus-framework/src/projection/status.rs` is *loop-state bookkeeping* — it only exists to track one loop's relationship between in-memory and persisted state for the flush-on-shutdown. That is runtime concern, on Agency's side of the line. Two outcomes are both forbidden by the design rule "no duplication, no drift, nexus is not a runtime":
- Keep the FSM in nexus → nexus becomes a (mini) runtime.
- Keep it only in the example → Agency reimplements it → drift.

So the FSM goes. The thing that must never drift — the *decision logic* — is already factored into two pure primitives (`Projector` + `PersistTrigger`) that live once in nexus and that both the example loop and Agency's actor call. Two different loops calling the same primitives is not duplication; it is each repo owning its own runtime.

**Tech Stack:** Rust 2024 edition (nexus workspace). Build/check: `nix develop -c nix flake check`. Feature branch + squash-merge via `gh pr merge --squash --delete-branch`. Signed commits required.

---

## File Structure

**Created:**
- `examples/projection-tokio/Cargo.toml` — example crate manifest.
- `examples/projection-tokio/src/lib.rs` — `run_projection`: the direct (no-FSM) tokio loop over the four primitives.
- `examples/projection-tokio/src/main.rs` — thin binary entry pointing at the tests.
- `examples/projection-tokio/tests/loop_tests.rs` — end-to-end loop tests ported from the deleted crate's `tests/projection_tests.rs`.

**Modified:**
- `Cargo.toml` (workspace root) — remove `"crates/nexus-framework"`, add `"examples/projection-tokio"`.
- `README.md:65` — drop the `nexus-framework` row; describe projection as primitives + example.
- `CLAUDE.md:29,76,92-101` — fix dependency graph, fix the `Projector` bullet, replace the framework section with a "projection primitives, consumer-owned loop" note that records the design intent.
- `crates/nexus-store/src/lib.rs:57` — fix the doc line that points at the deleted runner.

**Deleted:**
- `crates/nexus-framework/` — the entire crate, including the FSM (`status.rs`), the `Projection` typestate (`mod.rs`), `builder.rs`, and `error.rs`.

**Not changed:** `crates/nexus-store/src/projection.rs` keeps only the existing `Projector` trait. No FSM is added to the library.

---

## Phase A — Ship one direct loop as an example

### Task A1: Scaffold the example crate

**Files:**
- Create: `examples/projection-tokio/Cargo.toml`
- Create: `examples/projection-tokio/src/main.rs`
- Modify: `Cargo.toml` (workspace root) `members`

- [ ] **Step 1: Write the manifest**

```toml
[package]
name = "nexus-example-projection-tokio"
version = "0.0.0"
edition.workspace = true
publish = false

[dependencies]
bytes.workspace = true
futures.workspace = true
nexus = { path = "../../crates/nexus", features = ["derive"] }
nexus-store = { path = "../../crates/nexus-store", features = ["projection", "testing"] }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
workspace-hack = { version = "0.1", path = "../../crates/workspace-hack" }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }

[lints]
workspace = true
```

- [ ] **Step 2: Add a thin binary entry**

`examples/projection-tokio/src/main.rs`:

```rust
fn main() {
    println!(
        "Projection is a consumer-owned loop over nexus-store primitives. \
         See `cargo test -p nexus-example-projection-tokio` and src/lib.rs."
    );
}
```

- [ ] **Step 3: Register in the workspace**

In root `Cargo.toml` `members`: remove the line `"crates/nexus-framework",` and add `"examples/projection-tokio",`. (The crate directory is still on disk; Phase B deletes it. Removing it from `members` now keeps the workspace resolvable as the example is added — and the crate, being depended on by nothing, resolves fine while orphaned for one phase.)

- [ ] **Step 4: Verify it resolves**

Run: `nix develop -c cargo check -p nexus-example-projection-tokio`
Expected: PASS (compiles the placeholder `main.rs`; `lib.rs` is added next).

### Task A2: Implement the direct loop (no FSM)

**Files:**
- Create: `examples/projection-tokio/src/lib.rs`

This replaces the deleted `Projection::run()` (`nexus-framework/src/projection/mod.rs:235-307`) and its `ProjectionStatus` FSM with the direct form: hydrate → subscribe-from-checkpoint → `tokio::select!` loop folding with `Projector` and committing per `PersistTrigger` → flush tail on shutdown. State is tracked with two plain locals (`state`, `last_commit`) and an `uncommitted` marker — no Idle/Pending/Committed enum.

- [ ] **Step 1: Write the loop**

```rust
//! Consumer-owned projection loop over `nexus_store` primitives.
//!
//! nexus deliberately ships **no** event-loop runner — that is runtime,
//! and the runtime is the consumer's. nexus ships only the pure
//! primitives: a [`Projector`] (how to fold), a [`PersistTrigger`]
//! (when to persist), a [`Subscription`] (the cursor), and a
//! [`SnapshotStore`] (atomic `(state, position)` commit). This function
//! is one concrete loop wiring them under tokio. The Agency project
//! writes its own loop — a Zenoh-native actor that owns its cursor
//! lifecycle (Agency #138) — calling the same four primitives. Two
//! loops, no shared loop code, nothing to drift.

use std::future::Future;
use std::iter;
use std::num::NonZeroU32;

use futures::StreamExt;
use nexus::{DomainEvent, Id, Version};
use nexus_store::state::{PersistTrigger, SnapshotStore};
use nexus_store::subscription::RawSubscription;
use nexus_store::{Decode, EventStream, Projector, Subscription};

/// Run a projection loop until `shutdown` resolves or the stream errors.
///
/// 1. Hydrate the snapshot to resolve `(state, last_commit)` atomically;
///    if nothing is persisted, start from `projector.initial()`.
/// 2. Subscribe from `last_commit` (the cursor never returns `None`).
/// 3. For each event: fold via `Projector`; if `PersistTrigger` fires,
///    `commit` state and position together and advance `last_commit`.
/// 4. On shutdown, flush any uncommitted fold once before returning.
///
/// # Errors
/// Propagates snapshot-store (hydrate/commit), event-codec (decode),
/// projector (apply), and subscription/stream errors via the boxed error;
/// each preserves its `#[source]` chain in `to_string()`.
#[allow(
    clippy::too_many_arguments,
    reason = "example: explicit primitive wiring is clearer than a config struct"
)]
pub async fn run_projection<I, S, SS, P, EC, Trig>(
    id: I,
    subscription: Subscription<S>,
    snapshot_store: SS,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    schema_version: NonZeroU32,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    I: Id + Clone + Send + Sync,
    S: RawSubscription,
    S::Stream: Send + Unpin,
    SS: SnapshotStore<P::State, Version> + Send + Sync,
    P: Projector + Send + Sync,
    P::State: Send,
    for<'a> EC: Decode<P::Event, Output<'a> = P::Event> + Send + Sync,
    Trig: PersistTrigger + Send + Sync,
{
    // 1. Hydrate: starting state + last committed position, atomically.
    let (mut state, mut last_commit) = match snapshot_store
        .hydrate(&id, schema_version)
        .await?
    {
        Some((version, state)) => (state, Some(version)),
        None => (projector.initial(), None),
    };

    // 2. Subscribe from the checkpoint.
    let mut stream = subscription.subscribe(&id, last_commit).await?;

    // 3. Drive until shutdown or stream end.
    let mut uncommitted: Option<Version> = None;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            next = stream.next() => {
                let Some(item) = next else { break };
                let env = item?;
                let version = env.version();
                let event = event_codec.decode(&env)?;
                state = projector.apply(state, &event)?;
                if trigger.should_persist(last_commit, version, iter::once(event.name())) {
                    snapshot_store.commit(&id, schema_version, version, &state).await?;
                    last_commit = Some(version);
                    uncommitted = None;
                } else {
                    uncommitted = Some(version);
                }
            }
        }
    }

    // 4. Flush tail: commit the last uncommitted fold once.
    if let Some(version) = uncommitted {
        snapshot_store.commit(&id, schema_version, version, &state).await?;
    }
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles clean**

Run: `nix develop -c cargo clippy -p nexus-example-projection-tokio --all-targets -- -D warnings`
Expected: PASS — no warnings. If any name is not re-exported at the `nexus_store` crate root (the imports assume `Projector`, `Subscription`, `Decode`, `EventStream` at root and `PersistTrigger`/`SnapshotStore` under `::state`), switch to its module path rather than adding a re-export, and note it for the CLAUDE.md update in Phase B.

### Task A3: Port the end-to-end loop tests

**Files:**
- Create: `examples/projection-tokio/tests/loop_tests.rs`
- Source: `crates/nexus-framework/tests/projection_tests.rs` (14 `#[tokio::test]` cases + fixtures)

These cover behavior the deleted FSM unit tests did not: real subscription catch-up, resume-from-checkpoint across a restart, graceful-shutdown flush, schema-bump rebuild, and codec/projector error propagation. They move to the example — the new home of the loop. (The 6 FSM transition unit tests in the old `status.rs` are *not* ported: they tested the deleted bookkeeping enum, and the transitions they covered are now exercised end-to-end through `run_projection`.)

- [ ] **Step 1: Copy fixtures + tests verbatim as a starting point**

```bash
git show HEAD:crates/nexus-framework/tests/projection_tests.rs > examples/projection-tokio/tests/loop_tests.rs
```

- [ ] **Step 2: Rewire imports**

Replace the runner imports with the example loop:
- Remove `use nexus_framework::projection::{Projection, ProjectionError, ProjectionStatus, StartupDecision};`.
- Add `use nexus_example_projection_tokio::run_projection;`.
- Keep the existing fixture imports (`nexus::{DomainEvent, Message, Version}`, `nexus_store::testing::InMemoryStore`, `nexus_store::{... Store, InMemorySnapshotStore ...}`, `nexus_store::{EveryNEvents, Projector}`).

- [ ] **Step 3: Rewrite each call site from typestate to `run_projection`**

Every test currently builds `Projection::builder(...).build().initialize().await?.run(shutdown).await`. Replace with a direct call, constructing the subscription from the store handle:

```rust
run_projection(
    id.clone(),
    Subscription::new(&store),
    snapshot_store,
    projector,
    codec,
    trigger,
    schema_version,
    shutdown,
)
.await
```

Add `use nexus_store::Subscription;` to the test imports.

- [ ] **Step 4: Convert label/error assertions to outcome assertions**

- Tests that asserted `StartupDecision::{Fresh,Resume,Rebuild}` (`runner_first_run_..._is_not_rebuild`, `runner_rebuilds_from_beginning_on_schema_version_bump`, `runner_resumes_normally_after_rebuild_completes`): the label is gone with the typestate. Assert the *observable outcome* instead — after the loop, hydrate the snapshot store and assert the committed `(position, state)`. For the schema-bump "rebuild" case, run `run_projection` with the bumped `schema_version`; `hydrate` returns `None` for that version, so the loop starts from `initial()` and re-folds — assert the resulting state equals a full replay from the beginning.
- Tests that matched `ProjectionError::EventCodec(_)` / `ProjectionError::Projector(_)`: assert `result.is_err()` and that `result.unwrap_err().to_string()` contains the expected source message (the boxed error preserves the `#[source]` chain — the codec/projector messages survive).
- Tests that inspected `ProjectionStatus` directly: assert via hydrate on the snapshot store instead.

- [ ] **Step 5: Run the ported tests**

Run: `nix develop -c cargo test -p nexus-example-projection-tokio`
Expected: PASS — all ported loop tests green (`runner_processes_events_and_checkpoints`, `runner_resumes_from_checkpoint`, `runner_trigger_controls_checkpoint_frequency`, `runner_rebuilds_from_beginning_on_schema_version_bump`, `runner_graceful_shutdown_flushes_dirty_state`, `runner_catches_up_and_processes_all_existing_events`, the two error tests, etc.).

- [ ] **Step 6: Commit Phase A (independently green)**

```bash
git add examples/projection-tokio Cargo.toml
git commit -S -m "docs(example): consumer-owned projection loop over nexus-store primitives"
```

---

## Phase B — Delete the crate and update docs

### Task B1: Delete `nexus-framework`

**Files:**
- Delete: `crates/nexus-framework/` (entire directory)

- [ ] **Step 1: Remove the crate**

```bash
git rm -r crates/nexus-framework
```

- [ ] **Step 2: Confirm nothing else references it**

Run: `grep -rn "nexus-framework\|nexus_framework" --include=*.toml --include=*.rs .`
Expected: matches only inside `docs/plans/` (dated historical records — leave them). No matches in `Cargo.toml`, `crates/`, `examples/`, or `crates/nexus-store/src/`.

- [ ] **Step 3: Regenerate and verify workspace-hack**

Run: `nix develop -c cargo hakari generate && nix develop -c cargo hakari verify`
Expected: PASS — hakari reflects the removed crate and the added example.

### Task B2: Update docs and design intent

**Files:**
- Modify: `README.md:65`
- Modify: `CLAUDE.md:29` (graph), `CLAUDE.md:76` (Projector bullet), `CLAUDE.md:92-101` (framework section)
- Modify: `crates/nexus-store/src/lib.rs:57`

- [ ] **Step 1: README — replace the framework row**

Remove the `| nexus-framework | Runtime layer ... |` row. Add one sentence: "Projection is provided as primitives (`Projector`, `PersistTrigger`, `Subscription`, `SnapshotStore`); nexus ships no event-loop runner — the loop is the consumer's. See `examples/projection-tokio`."

- [ ] **Step 2: CLAUDE.md — dependency graph (line ~29)**

Replace `nexus-framework --> nexus-store --> nexus (kernel)` with `nexus-store --> nexus (kernel)` (keep the `nexus-fjall --> nexus-store` and `nexus-macros <-- nexus` lines).

- [ ] **Step 3: CLAUDE.md — replace the framework section (lines ~92-101)**

Replace the entire `### Framework Crate (nexus-framework) — Batteries-Included Runtime Layer` section with a short note under the store crate, recording the design intent (per the "document design philosophy in CLAUDE.md" rule):

> ### Projection — primitives only, no runner
>
> nexus deliberately ships **no** projection runner and **no** event loop. A read-model projection is built from four pure primitives that already live in `nexus-store`: `Projector` (the fallible fold), `PersistTrigger` (when to persist), `Subscription` (the cursor), and `SnapshotStore` (atomic `(state, position)` commit). The loop that wires them — and all runtime concerns (lifecycle, supervision, passivation, cursor management) — is owned by the consumer, never by nexus; shipping a loop here would make nexus a runtime and would duplicate logic that belongs to the runtime layer. In the Nexus + Agency product, that runtime is Agency's Zenoh-native actor framework (a kameo fork); a tokio reference loop lives in `examples/projection-tokio`. The earlier `nexus-framework` crate (a `tokio::select!` runner + an Idle/Pending/Committed FSM) was retired on 2026-06-17 for exactly this reason — the FSM was loop bookkeeping, not an event-sourcing primitive.

- [ ] **Step 4: CLAUDE.md — Projector bullet (line ~76)**

Change "The IO-driven runner lives in `nexus-framework`." to "nexus ships no runner; the IO-driven loop is the consumer's. See `examples/projection-tokio`."

- [ ] **Step 5: nexus-store lib.rs doc (line ~57)**

Change "The IO-driven runner lives in `nexus-framework`." to "nexus ships no runner; the loop is consumer-owned (see `examples/projection-tokio`)."

- [ ] **Step 6: Full gate**

Run: `nix develop -c nix flake check`
Expected: PASS — clippy/fmt/tests/taplo/audit/deny/hakari green across the workspace (without `nexus-framework`, with the new example).

- [ ] **Step 7: Commit Phase B**

```bash
git add -A
git commit -S -m "refactor!: retire nexus-framework; projection is primitives, the loop is the consumer's"
```

---

## Phase C — Reconcile the GitHub cards (do NOT auto-execute)

> Outward-facing, hard-to-reverse changes across **two** repos. Present the proposed edits and get explicit confirmation before any `gh` mutation. List only.

- [ ] **nexus #156 "Projection Subtask 2: Inline projection execution (same-transaction)"** — propose closing as "won't do": presumes nexus owns a runner. With no runner, inline same-transaction projection is the consumer/actor's concern (Agency #138). Link Agency #138 in the closing comment.
- [ ] **nexus #124 "Add projections / read model infrastructure"** — propose rescoping to "Projection = primitives (`Projector`/`PersistTrigger`/`Subscription`/`SnapshotStore`) + `examples/projection-tokio`; delete `nexus-framework`." This plan implements it; close on merge.
- [ ] **Agency cross-links** (`devrandom-labs/agency`): comment on **#137** (nexus-store mapping for keri-store) and **#138** (AID-keyed cursor passivation/resume) that nexus now exposes exactly the primitives a consumer-owned loop needs — fold via `Projector`, persist policy via `PersistTrigger`, atomic cursor+state via `SnapshotStore`, cursor via `Subscription` — and ships no competing runner; and on **#120** (fjall vs LMDB) that `nexus-fjall` is the candidate adapter. Confirm the correct `gh` account for the agency repo before posting.

---

## Self-Review

**1. Spec coverage.**
- "Delete nexus-framework including the FSM" → Phase B Task B1; the FSM is not relocated anywhere. ✓
- "nexus keeps only the four pure primitives" → unchanged `nexus-store` (`Projector` in projection.rs; `PersistTrigger`/`SnapshotStore` in state.rs; `Subscription` in subscription.rs) — Phase A adds nothing to the library. ✓
- "One demonstration loop, consumer-owned" → Phase A `run_projection` (direct form, no FSM). ✓
- "No lost behavior coverage" → 14 end-to-end tests ported (A3); 6 FSM unit tests intentionally dropped (they tested deleted bookkeeping; their transitions are covered end-to-end). ✓
- "Record design intent / no-runtime line" → Phase B Task B2 Step 3. ✓
- "Reconcile cards across both repos" → Phase C, gated on approval. ✓

**2. Placeholder scan.** `run_projection` (A2) is complete and compiles against the documented `nexus_store` surface. A3 specifies exact call-site and assertion rewrites (no "handle errors appropriately"). A3 Step 1 moves test code via `git show` rather than retyping. No TBD/TODO.

**3. Type consistency.** `trigger.should_persist(last_commit, version, iter::once(event.name()))` matches the `PersistTrigger::should_persist(&self, old: Option<Version>, new: Version, names: impl Iterator<Item: AsRef<str>>)` signature (confirmed in the old `status.rs` test fixtures); `event.name()` is `DomainEvent::name -> &'static str` (`&'static str: AsRef<str>`). `projector.apply(state, &event)` consumes `state` by value and returns the new state, so `P::State: Clone` is **not** required — the bound is correctly omitted (the old FSM runner required `Clone`; the direct loop does not). `snapshot_store.commit(&id, schema_version, version, &state)` and `hydrate(&id, schema_version) -> Option<(Version, State)>` match `SnapshotStore<P::State, Version>`. The `run_projection` signature in A2 matches the call site in A3 Step 3 argument-for-argument.

**4. Green-at-every-commit.** Nothing outside `nexus-framework` depends on it, so: Phase A (add example, drop framework from `members`) compiles with the framework dir orphaned-but-present; Phase B (delete dir, docs, hakari) compiles. Two clean signed commits, no flake-check bypass — respects the never-skip-flake-check and never-`--no-verify` rules.
