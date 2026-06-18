# Aggregate Redesign — Purist Decide on a Marker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dissolve the aggregate load→decide→save seam by making the aggregate a bare marker, implementing `Handle` on it as a pure `handle(state, cmd) -> events`, and dispatching from `AggregateRoot<A>::handle`. Delete `AggregateEntity`, `from_root`, and the macro-generated newtype.

**Architecture:** `Handle<C, N>`'s supertrait moves from `AggregateEntity` to `Aggregate`; its method becomes an associated `fn handle(state: &Self::State, cmd: C)` (no `&self`). `AggregateRoot<A>` gains an inherent `handle<C, N>(&self, cmd)` that calls `A::handle(self.state(), cmd)`. `Repository::load`/`save` keep their `AggregateRoot<A>` signatures unchanged — so the entire `nexus-store`/`nexus-fjall` facade is untouched except for test aggregates. The `#[nexus::aggregate]` macro shrinks to emit only `impl Aggregate`.

**Tech Stack:** Rust 2024, `nexus` kernel + `nexus-macros` proc-macro. Build/check via `nix develop -c …`. Toolchain rustc 1.96-nightly (2026-03-29) — the gate is `nix flake check` (pre-commit hook). Signed commits (`-S`). Branch `feat/aggregate-test-fixture` (current).

**Design doc:** `docs/plans/2026-06-17-aggregate-redesign-purist-decide-design.md` · **Issue:** #197

---

## File Structure

- `crates/nexus/src/aggregate.rs` — delete `AggregateEntity`; reshape `Handle`; add `AggregateRoot::handle`; `#[doc(hidden)]` the post-persist mutators; update 2 doc examples; add inline dispatch unit test.
- `crates/nexus/src/lib.rs` — drop `AggregateEntity` from the re-export list.
- `crates/nexus-macros/src/lib.rs` — gut `parse_aggregate` to emit only the unit struct + `impl Aggregate`.
- `crates/nexus-macros/tests/expand_roundtrip.rs` — rewrite the expected expansion.
- `crates/nexus-macros/tests/{macro_hygiene.rs,macro_property_tests.rs}` — drop `AggregateEntity`/newtype assumptions.
- `crates/nexus-macros/tests/cross_crate_test/src/lib.rs` — convert aggregate to marker + purist `Handle`.
- `crates/nexus-macros/README.md` — update macro-output docs.
- `crates/nexus/tests/kernel_tests/{aggregate_root_tests,integration_test,newtype_aggregate_tests}.rs` — convert.
- `crates/nexus-store/tests/*`, `crates/nexus-fjall/tests/*` (grep-driven) — convert any test aggregate that calls `handle` or names the newtype/`new`.
- `examples/inmemory/src/main.rs`, `examples/store-and-kernel/src/main.rs` — convert.
- `CLAUDE.md` — rewrite the aggregate-family description.

### The canonical transformation (apply everywhere)

**Hand-written aggregate (manual `Aggregate` + `AggregateEntity` impls):**
```rust
// BEFORE
struct Counter(AggregateRoot<Self>);
impl Aggregate for Counter { type State = CounterState; type Error = CounterError; type Id = CounterId; }
impl AggregateEntity for Counter {
    fn root(&self) -> &AggregateRoot<Self> { &self.0 }
    fn root_mut(&mut self) -> &mut AggregateRoot<Self> { &mut self.0 }
}
impl Handle<Increment> for Counter {
    fn handle(&self, cmd: Increment) -> Result<Events<CounterEvent>, CounterError> {
        if self.state().total == u64::MAX { return Err(CounterError::Overflow) }
        Ok(events![CounterEvent::Incremented(cmd.0)])
    }
}
// ... call site:
let mut c = Counter::new(id);          // or Counter(AggregateRoot::new(id))
c.root_mut().replay(v, &ev)?;
let decided = c.handle(Increment(5))?;

// AFTER
struct Counter;                                  // bare marker
impl Aggregate for Counter { type State = CounterState; type Error = CounterError; type Id = CounterId; }
// (no AggregateEntity impl — deleted)
impl Handle<Increment> for Counter {
    fn handle(state: &CounterState, cmd: Increment) -> Result<Events<CounterEvent>, CounterError> {
        if state.total == u64::MAX { return Err(CounterError::Overflow) }
        Ok(events![CounterEvent::Incremented(cmd.0)])
    }
}
// ... call site:
let mut c = AggregateRoot::<Counter>::new(id);
c.replay(v, &ev)?;
let decided = c.handle(Increment(5))?;           // inherent dispatch on AggregateRoot
```

**Macro aggregate (`#[nexus::aggregate(...)] struct X;`):** the attribute line is unchanged; only the `Handle` impl body and call sites change exactly as above (`self.state()` → `state`, `X::new(id)` → `AggregateRoot::<X>::new(id)`).

Transformation rules, mechanical:
1. `struct Name(AggregateRoot<Self>);` → `struct Name;` (only for hand-written; macro structs are already unit structs).
2. Delete every `impl AggregateEntity for Name { … }` block.
3. `impl Handle<C> for Name { fn handle(&self, cmd: C)` → `fn handle(state: &<NameState>, cmd: C)`; inside the body, `self.state()` → `state` (and `self.state().x` → `state.x`). No handler uses `self.id()`/`self.version()` (verified workspace-wide).
4. Construction `Name::new(id)` → `AggregateRoot::<Name>::new(id)`; `Name(AggregateRoot::new(id))` → `AggregateRoot::<Name>::new(id)`.
5. Replaying in tests `agg.root_mut().replay(...)` → `agg.replay(...)`; `agg.root_mut().apply_events(...)` → `agg.apply_events(...)`; `agg.state()`/`agg.version()`/`agg.id()` already exist on `AggregateRoot`.
6. `agg.handle(cmd)` is unchanged at the call site — it now resolves to the inherent `AggregateRoot::handle`.

---

## Task 1: Reshape the kernel (`aggregate.rs` + `lib.rs`)

**Files:**
- Modify: `crates/nexus/src/aggregate.rs`
- Modify: `crates/nexus/src/lib.rs`

- [ ] **Step 1: Write the failing inline dispatch test**

At the end of `crates/nexus/src/aggregate.rs`, add an inline test module that proves the new shape end-to-end without the macro and without `AggregateEntity`:

```rust
#[cfg(test)]
mod purist_dispatch_tests {
    use super::*;
    use crate::event::DomainEvent;
    use crate::events::{Events, events};
    use crate::id::Id;
    use crate::message::Message;

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct CtrId(u64);
    impl std::fmt::Display for CtrId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
    }
    impl AsRef<[u8]> for CtrId {
        fn as_ref(&self) -> &[u8] { Box::leak(self.0.to_le_bytes().to_vec().into_boxed_slice()) }
    }
    impl Id for CtrId {}

    #[derive(Clone, Debug, PartialEq)]
    enum CtrEvent { Added(u64) }
    impl Message for CtrEvent {}
    impl DomainEvent for CtrEvent { fn name(&self) -> &'static str { "Added" } }

    #[derive(Clone, Debug, Default, PartialEq)]
    struct CtrState { total: u64 }
    impl AggregateState for CtrState {
        type Event = CtrEvent;
        fn initial() -> Self { Self::default() }
        fn apply(mut self, e: &CtrEvent) -> Self {
            match e { CtrEvent::Added(n) => self.total += *n }
            self
        }
    }

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("ctr error")]
    struct CtrError;

    // Bare marker — no newtype, no AggregateEntity.
    struct Counter;
    impl Aggregate for Counter { type State = CtrState; type Error = CtrError; type Id = CtrId; }

    struct Add(u64);
    // Handle is on the marker; decide is a pure function of (state, command).
    impl Handle<Add> for Counter {
        fn handle(state: &CtrState, cmd: Add) -> Result<Events<CtrEvent>, CtrError> {
            if cmd.0 == 0 { return Err(CtrError) }
            let _ = state.total; // reads state, no mutation
            Ok(events![CtrEvent::Added(cmd.0)])
        }
    }

    #[test]
    fn aggregate_root_dispatches_to_marker_handle() {
        let root = AggregateRoot::<Counter>::new(CtrId(1));
        let decided = root.handle(Add(5)).expect("decide ok");
        assert_eq!(decided.as_slice(), &[CtrEvent::Added(5)]);
    }

    #[test]
    fn aggregate_root_dispatch_propagates_domain_error() {
        let root = AggregateRoot::<Counter>::new(CtrId(1));
        assert_eq!(root.handle(Add(0)), Err(CtrError));
    }
}
```

(If `Events` has no `as_slice`, use `decided.into_iter().collect::<Vec<_>>()` and compare to `vec![CtrEvent::Added(5)]`. Read `crates/nexus/src/events.rs` to confirm the accessor before writing.)

- [ ] **Step 2: Run it — verify it fails to compile**

Run: `nix develop -c cargo test -p nexus --lib purist_dispatch_tests 2>&1 | tail -20`
Expected: FAIL — `Handle` still requires `AggregateEntity`/`&self`; `AggregateRoot::handle` does not exist.

- [ ] **Step 3: Reshape the `Handle` trait**

In `crates/nexus/src/aggregate.rs`, replace the trait (currently ~lines 173-185):

```rust
pub trait Handle<C, const N: usize = 0>: Aggregate {
    /// Decide a command, returning decided events or a domain error.
    ///
    /// A **pure decision**: reads the aggregate's current `state` and the
    /// command, returns the decided events. It has no access to version or
    /// identity — a decision is a function of domain state and command only,
    /// never of persistence position. External data should be resolved by the
    /// application layer and passed as fields on the command `C`.
    ///
    /// Implemented on the aggregate marker type (e.g. `impl Handle<Withdraw>
    /// for BankAccount`). Invoke it via [`AggregateRoot::handle`] on a loaded
    /// aggregate, or directly as `BankAccount::handle(&state, cmd)`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the command violates a domain invariant.
    fn handle(state: &Self::State, cmd: C) -> Result<Events<EventOf<Self>, N>, Self::Error>;
}
```

Update the `Handle` doc example above the trait (the `CompleteTodo` example, ~lines 145-171): change `struct Todo(AggregateRoot<Self>);` to `struct Todo;`, delete its `impl AggregateEntity for Todo { … }` line, and change the impl to:
```rust
/// impl Handle<CompleteTodo> for Todo {
///     fn handle(state: &TodoState, _cmd: CompleteTodo) -> Result<Events<TodoEvent>, TodoError> {
///         if state.done {
///             return Err(TodoError);
///         }
///         Ok(events![TodoEvent::Completed])
///     }
/// }
```

- [ ] **Step 4: Delete the `AggregateEntity` trait**

Remove the entire `pub trait AggregateEntity: Aggregate { … }` block (currently ~lines 187-227, from its doc comment through its closing brace). In the `Aggregate` doc example (~lines 104-115), change `struct MyAggregate(AggregateRoot<Self>);` to `struct MyAggregate;` and delete the `impl AggregateEntity for MyAggregate { … }` lines.

- [ ] **Step 5: Add the `AggregateRoot::handle` dispatch + `#[doc(hidden)]` the mutators**

Inside `impl<A: Aggregate> AggregateRoot<A>` in `crates/nexus/src/aggregate.rs`, add:

```rust
    /// Decide a command against the current state.
    ///
    /// Dispatches to the aggregate's [`Handle`] impl, passing the current
    /// [`state`](Self::state). Pure — reads state, returns decided events,
    /// mutates nothing. After persisting the returned events via the
    /// repository, the aggregate's state advances.
    ///
    /// # Errors
    ///
    /// Returns `A::Error` when the command violates a domain invariant.
    pub fn handle<C, const N: usize>(&self, cmd: C) -> Result<Events<EventOf<A>, N>, A::Error>
    where
        A: Handle<C, N>,
    {
        A::handle(self.state(), cmd)
    }
```

Add `#[doc(hidden)]` to each of `replay`, `advance_version`, `apply_events`, and `apply_event` (keep them `pub` — `nexus-store` calls them across the crate boundary), and append one line to each doc comment: `Internal — driven by the repository; not part of the user-facing flow.`

- [ ] **Step 6: Drop `AggregateEntity` from the crate re-exports**

In `crates/nexus/src/lib.rs`, the `pub use aggregate::{ … }` block (lines 9-12) lists `AggregateEntity`. Remove that one identifier, leaving:
```rust
pub use aggregate::{
    Aggregate, AggregateRoot, AggregateState, DEFAULT_MAX_REHYDRATION_EVENTS, EventOf, Handle,
};
```

- [ ] **Step 7: Run the inline test + lib build + doctests**

Run: `nix develop -c cargo test -p nexus --lib purist_dispatch_tests 2>&1 | tail -20`
Expected: PASS (both dispatch tests).
Run: `nix develop -c cargo test -p nexus --doc 2>&1 | tail -20`
Expected: PASS (the two reshaped doc examples compile).

Note: `cargo test -p nexus` (integration tests) will NOT compile yet — the `tests/kernel_tests/*` still use `AggregateEntity`. That is fixed in Task 3. Verify only `--lib` and `--doc` here.

- [ ] **Step 8: Clippy (lib) + commit**

Run: `nix develop -c cargo clippy -p nexus --lib --all-features -- -D warnings 2>&1 | tail -20`
Expected: clean.

```bash
git add crates/nexus/src/aggregate.rs crates/nexus/src/lib.rs
git commit -S -m "feat(kernel)!: purist decide — Handle on the marker, AggregateRoot::handle dispatch

Handle<C,N> moves from AggregateEntity to Aggregate and becomes a pure
associated fn handle(state: &State, cmd) -> events. AggregateRoot<A> gains
an inherent handle() that dispatches to A::handle(self.state(), cmd), so a
loaded aggregate is directly decidable. AggregateEntity is deleted; the
post-persist mutators are doc(hidden). Repository load/save signatures are
unchanged. Part of #197.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Gut the `aggregate` macro + fix its tests

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs`
- Modify: `crates/nexus-macros/tests/expand_roundtrip.rs`
- Modify: `crates/nexus-macros/tests/macro_hygiene.rs`
- Modify: `crates/nexus-macros/tests/macro_property_tests.rs`
- Modify: `crates/nexus-macros/README.md`

- [ ] **Step 1: Shrink `parse_aggregate` to emit only `impl Aggregate`**

In `crates/nexus-macros/src/lib.rs`, replace the `expanded` token stream in `parse_aggregate` (currently ~lines 510-548, which emits the newtype struct + `Aggregate` + `AggregateEntity` + `new` + `Debug`) with:

```rust
    let expanded = quote! {
        #(#user_attrs)*
        #vis struct #name;

        impl ::nexus::Aggregate for #name {
            type State = #state_type;
            type Error = #error_type;
            type Id = #id_type;
        }
    };
```

(The struct stays a unit marker — no `AggregateRoot` field, no `AggregateEntity` impl, no `new`, no custom `Debug`. The macro still validates "unit struct only" up front, which now matches the emitted shape exactly.) Update the macro's top doc comment (~lines 7-20) to describe the new output: "Generates `impl Aggregate` for the unit struct. The aggregate is a marker; construct instances as `AggregateRoot::<Name>::new(id)` and implement `Handle<C>` on the marker."

- [ ] **Step 2: Rewrite the expansion roundtrip test**

`crates/nexus-macros/tests/expand_roundtrip.rs` asserts the macro output equals a hand-written expansion. Read it, then replace the expected expansion so it matches Step 1: a unit `struct RtAggregate;` plus only the `impl ::nexus::Aggregate for RtAggregate` block (delete the expected newtype field, the `AggregateEntity` impl, the `new`, and the `Debug` impl). Keep whatever token-stream comparison harness the file already uses.

- [ ] **Step 2b: Run the roundtrip test**

Run: `nix develop -c cargo test -p nexus-macros --test expand_roundtrip 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Fix `macro_hygiene.rs` and `macro_property_tests.rs`**

Read both. They construct macro aggregates and exercise `AggregateEntity`/newtype behavior (`.root()`, `Name::new(id)`, the `Name(AggregateRoot…)` field). Apply the canonical transformation: drop any `AggregateEntity`/`.root()`/`.root_mut()` assertions, change `Name::new(id)` → `AggregateRoot::<Name>::new(id)`, and any `Handle` impls to the purist `handle(state, cmd)` form. If a property test asserted "macro generates `new`/`AggregateEntity`", replace that assertion with "macro generates `impl Aggregate`" (e.g. construct an `AggregateRoot::<Name>` and read `state()`/`id()`).

- [ ] **Step 4: Update the macro README**

In `crates/nexus-macros/README.md`, update the `#[nexus::aggregate]` section: remove mentions of the generated newtype, `AggregateEntity`, and `new`; state it generates `impl Aggregate` on a marker and that `Handle<C>` is implemented on the marker as `handle(state, cmd)`.

- [ ] **Step 5: Run the full macro crate tests + clippy**

Run: `nix develop -c cargo test -p nexus-macros --all-features 2>&1 | tail -25`
Expected: PASS (note `cross_crate_test` is a nested crate fixed in Task 4 — if it is compiled by this command and fails, defer it: run `--test expand_roundtrip --test macro_hygiene --test macro_property_tests` explicitly here and fix `cross_crate_test` in Task 4).
Run: `nix develop -c cargo clippy -p nexus-macros --all-targets --all-features -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-macros/src/lib.rs crates/nexus-macros/tests/expand_roundtrip.rs crates/nexus-macros/tests/macro_hygiene.rs crates/nexus-macros/tests/macro_property_tests.rs crates/nexus-macros/README.md
git commit -S -m "feat(macros)!: aggregate macro emits only impl Aggregate on a marker

Drops the generated newtype, AggregateEntity impl, new(), and Debug.
Updates roundtrip/hygiene/property tests and README. Part of #197.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Convert the kernel integration tests

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/aggregate_root_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/integration_test.rs`
- Modify: `crates/nexus/tests/kernel_tests/newtype_aggregate_tests.rs`

- [ ] **Step 1: Convert `aggregate_root_tests.rs`**

Read the file. Its `Counter` is a hand-written newtype with manual `Aggregate` + `AggregateEntity` + `Handle` impls. Apply the canonical transformation (struct → marker, delete `AggregateEntity`, `Handle` bodies `self.state()` → `state`, construction `Counter::new` → `AggregateRoot::<Counter>::new`, `c.root_mut().replay/apply_events` → `c.replay/apply_events`, `c.handle(cmd)` unchanged). Keep every assertion and test name.

- [ ] **Step 2: Convert `integration_test.rs`** (the `User` aggregate) — same transformation.

- [ ] **Step 3: Repurpose `newtype_aggregate_tests.rs`**

This file's reason for existing was the newtype/`AggregateEntity` delegation, which is gone. Read it. Convert the `UserAggregate` to a marker and the `Handle` impls to purist form; for any test that asserted delegation (`agg.state()` forwards to `agg.root().state()`, etc.), rewrite it to assert the equivalent on `AggregateRoot<UserAggregate>` directly (e.g. `root.state()` after replay/dispatch). Rename the file's module doc comment to reflect "marker aggregate + dispatch" rather than "newtype". Do not delete tests that still carry meaning (replay correctness, decide correctness) — only rephrase the ones that tested the deleted delegation layer. If, after conversion, a test is a pure duplicate of one in `aggregate_root_tests.rs`, delete it rather than leave a redundant copy (CLAUDE.md: each invariant tested once).

- [ ] **Step 4: Run the full kernel test suite**

Run: `nix develop -c cargo test -p nexus --all-features 2>&1 | tail -30`
Expected: PASS (lib, doc, and all three integration test files).

- [ ] **Step 5: Clippy (all targets) + commit**

Run: `nix develop -c cargo clippy -p nexus --all-targets --all-features -- -D warnings 2>&1 | tail -20`
Expected: clean.

```bash
git add crates/nexus/tests/kernel_tests/
git commit -S -m "test(kernel): convert aggregate tests to marker + purist decide

Part of #197.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Convert downstream test aggregates (macros cross-crate, store, fjall)

**Files (grep-driven — confirm the exact set first):**
- Modify: `crates/nexus-macros/tests/cross_crate_test/src/lib.rs`
- Modify: any `crates/nexus-store/tests/*.rs` / `crates/nexus-fjall/tests/*.rs` that define an aggregate and call `.handle(...)` or name the newtype/`new`.

- [ ] **Step 1: Enumerate the affected files**

Run: `nix develop -c bash -c 'grep -rln "AggregateEntity\|#\[nexus::aggregate\|\.handle(" crates/nexus-store crates/nexus-fjall crates/nexus-macros/tests/cross_crate_test'`
Record the list. The store facade tests historically build raw event slices and `save` them without calling `handle`; those only need changes if they name the deleted newtype field or `Name::new`. Confirm per file by reading it.

- [ ] **Step 2: Convert `cross_crate_test/src/lib.rs`** — apply the canonical transformation to its `TaskAggregate` and its `Handle` impls.

- [ ] **Step 3: Convert each store/fjall test that calls `handle` or names the newtype** — apply the canonical transformation. For tests that only `load`/`save` raw events (no `handle`), the only likely change is `Name::new(id)` → `AggregateRoot::<Name>::new(id)` (if present) and the macro aggregate's `Handle` impls (if present).

- [ ] **Step 4: Run the affected crates' tests**

Run: `nix develop -c cargo test -p nexus-macros -p nexus-store -p nexus-fjall --all-features 2>&1 | tail -40`
Expected: PASS.

- [ ] **Step 5: Clippy + commit**

Run: `nix develop -c cargo clippy -p nexus-macros -p nexus-store -p nexus-fjall --all-targets --all-features -- -D warnings 2>&1 | tail -20`
Expected: clean.

```bash
git add crates/nexus-macros/tests/cross_crate_test crates/nexus-store/tests crates/nexus-fjall/tests
git commit -S -m "test(store,fjall,macros): convert downstream aggregates to marker + purist decide

Part of #197.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Convert the examples

**Files:**
- Modify: `examples/inmemory/src/main.rs`
- Modify: `examples/store-and-kernel/src/main.rs`

- [ ] **Step 1: Convert `examples/store-and-kernel/src/main.rs`**

Read it. Its `BankAccount` is a macro aggregate driven by-hand: `BankAccount::new(id)`, `account.handle(cmd)`, `account.root_mut().apply_events(&decided)`, `account.root_mut().advance_version(v)`. Apply the canonical transformation: `BankAccount::new(id)` → `AggregateRoot::<BankAccount>::new(id)`; `account.handle(cmd)` unchanged; `account.root_mut().apply_events(...)` → `account.apply_events(...)`; `account.root_mut().advance_version(v)` → `account.advance_version(v)`; `Handle` impls → purist `handle(state, cmd)`.

- [ ] **Step 2: Convert `examples/inmemory/src/main.rs`**

Read it. It uses a local hand-rolled `InMemoryStore` whose `load() -> BankAccount`. Convert `BankAccount` to a marker + purist `Handle`. The local store's `load`/`save` should now produce/consume `AggregateRoot<BankAccount>` (build via `AggregateRoot::<BankAccount>::new(id)` + `replay`, and decide via `account.handle(cmd)`). Keep the example's narrative comments accurate.

- [ ] **Step 3: Build + run both examples**

Run: `nix develop -c cargo run -p store-and-kernel 2>&1 | tail -15`
Expected: runs to completion, prints its demo output, exit 0.
Run: `nix develop -c cargo run -p inmemory 2>&1 | tail -15`
Expected: runs to completion, exit 0.
(If the example package names differ, discover them via `nix develop -c cargo metadata --no-deps --format-version 1 | grep -o '"name":"[^"]*"' | sort -u`.)

- [ ] **Step 4: Clippy (examples) + commit**

Run: `nix develop -c cargo clippy -p inmemory -p store-and-kernel --all-targets -- -D warnings 2>&1 | tail -20`
Expected: clean.

```bash
git add examples/inmemory examples/store-and-kernel
git commit -S -m "docs(examples): drive load→decide→save via AggregateRoot::handle

Part of #197.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Docs + full gate

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Rewrite the aggregate-family description in `CLAUDE.md`**

In the `aggregate.rs` bullet under "Kernel Crate", remove `AggregateEntity` and the newtype-delegation narrative. New description: "`Aggregate` trait (binds State + Error + Id) is implemented on a **bare marker** type. `AggregateRoot<A>` is the read-only state container (replay, version tracking, panic-safe apply) and carries the inherent `handle<C,N>(&self, cmd)` dispatch. `Handle<C, const N>` is the pure decide function `handle(state: &State, cmd) -> Events`, implemented on the marker. There is no entity newtype and no `AggregateEntity`/`from_root`." Update the "Aggregate Lifecycle (Key Flow)" section: step 1 becomes "Define aggregate via `#[nexus::aggregate]` on a unit struct (emits `impl Aggregate`) or implement `Aggregate` manually; implement `Handle<C,N>` on the marker"; step 4 becomes "Decide: `root.handle(command)` dispatches to `Aggregate::handle(state, command)`". Update the `nexus-macros` section: the `aggregate` macro now emits only `impl Aggregate`.

- [ ] **Step 2: Grep for stragglers**

Run: `nix develop -c bash -c 'grep -rn "AggregateEntity\|from_root\|root_mut()\|\.root()" crates examples CLAUDE.md --include=*.rs --include=*.md | grep -v "docs/plans/"'`
Expected: no hits outside design/plan docs. Fix any straggler with the canonical transformation.

- [ ] **Step 3: Full gate**

Run: `nix develop -c nix flake check 2>&1 | tail -30`
Expected: PASS across the workspace (clippy deny, fmt, taplo, nextest, audit, deny, hakari).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -S -m "docs: document marker-based aggregate + purist decide (#197)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Rebase the #126 fixture plan

**Files:**
- Modify: `docs/plans/2026-06-17-aggregate-test-fixture-plan.md`

- [ ] **Step 1: Delete the `from_root` task and rewrite the fixture around `AggregateRoot`**

In the #126 plan, delete Task 1 (add `from_root`) entirely. Update the fixture design so `given` builds `AggregateRoot::<A>::new(id)` and replays via `root.replay(...)`, and `when` calls `root.handle(cmd)` (the inherent dispatch). The sample aggregate becomes a marker; its `Handle` impls become `handle(state, cmd)`. Update the architecture paragraph to drop the `AggregateEntity::from_root` mechanism. (No code is executed in this task — it re-points the downstream plan. The fixture itself is implemented later under #126.)

- [ ] **Step 2: Commit**

```bash
git add docs/plans/2026-06-17-aggregate-test-fixture-plan.md
git commit -S -m "docs: rebase #126 fixture plan onto marker aggregate (drop from_root)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (design doc → task):
- Reshape `Handle` to `handle(state, cmd)` on marker → Task 1 ✓
- `AggregateRoot::handle` dispatch → Task 1 (with inline TDD test) ✓
- Delete `AggregateEntity`/newtype/`from_root` → Task 1 (trait), Task 2 (macro) ✓
- `#[doc(hidden)]` post-persist mutators (light footgun reduction) → Task 1 Step 5 ✓
- Repository/facade signatures unchanged → verified by Task 4 (store/fjall tests pass without facade edits) ✓
- Gut macro → Task 2 ✓
- Convert all hand-written impls/examples/cross-crate/docs → Tasks 3–6 ✓
- Reshape #126 → Task 7 ✓
- Non-goals (branded lifetimes, typestate, facade change) → none added ✓
- Hard capability seal → explicitly deferred to a separate issue (design doc); not in this plan ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases". The conversion tasks reference the single canonical transformation (defined once in File Structure) with a fully worked before/after, rather than repeating per file — each conversion task names exact files and the rules to apply. The one genuine unknown (`Events` accessor name) has an explicit "read events.rs to confirm" instruction with a fallback.

**3. Type consistency:** `Handle::handle(state: &Self::State, cmd: C) -> Result<Events<EventOf<Self>, N>, Self::Error>` is identical in the trait (Task 1.3), the dispatch caller `A::handle(self.state(), cmd)` (Task 1.5), the inline test (Task 1.1), and the canonical transformation rule. `AggregateRoot::handle<C, const N: usize>(&self, cmd)` matches between definition and all call sites (`root.handle(cmd)`). Construction `AggregateRoot::<Name>::new(id)` is uniform across Tasks 3–5. The macro output (`struct Name; impl Aggregate`) matches between Task 2.1 (emit) and Task 2.2 (expected expansion).
