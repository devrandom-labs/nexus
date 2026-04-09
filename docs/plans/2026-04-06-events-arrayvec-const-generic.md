# Events ArrayVec + Const Generic Capacity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `SmallVec` with `ArrayVec` in `Events<E>`, add const generic `N` for compile-time capacity declaration, and enforce max event count on the `Handle` trait — eliminating heap allocation and enabling `no_std`/`no_alloc` IoT use.

**Architecture:** `Events<E, const N: usize = 0>` stores `first: E` + `rest: ArrayVec<E, N>`, giving total capacity N+1. The `Handle<C, const N: usize = 0>` trait's const generic declares the handler's max additional events. N=0 default means single-event handlers (the common case) need no annotation. The `events!` macro leaves N for type inference from the return position.

**Tech Stack:** `arrayvec` 0.7 (`no_std`, `default-features = false`), Rust const generics (stable)

---

## Design Summary

```rust
// Struct: first is guaranteed, rest holds 0..=N additional events
pub struct Events<E: DomainEvent, const N: usize = 0> {
    first: E,
    rest: ArrayVec<E, N>,
}
// Total capacity = N + 1. N=0 means exactly 1 event.

// Handle: N declares max additional events this handler can return
pub trait Handle<C, const N: usize = 0>: AggregateEntity {
    fn handle(&self, cmd: C) -> Result<Events<EventOf<Self>, N>, <Self as Aggregate>::Error>;
}

// Usage: most handlers produce 1 event, N defaults to 0
impl Handle<Increment> for Counter {
    fn handle(&self, _cmd: Increment) -> Result<Events<CounterEvent>, CounterError> {
        Ok(events![CounterEvent::Incremented])
    }
}

// Multi-event handler declares N=2 (up to 3 events)
impl Handle<PlaceOrder, 2> for Order {
    fn handle(&self, cmd: PlaceOrder) -> Result<Events<OrderEvent, 2>, OrderError> {
        if self.state().express {
            Ok(events![OrderEvent::Placed(cmd.item)])           // 1 of 3
        } else {
            Ok(events![OrderEvent::Placed(cmd.item),
                       OrderEvent::Reserved, OrderEvent::Started]) // 3 of 3
        }
    }
}
```

**Key design decisions:**
- N = additional capacity beyond `first` (not total). Required for stable Rust — `N-1` in const position needs unstable `generic_const_exprs`.
- `events!` macro does NOT set N explicitly — it relies on type inference from the return type. In let-bindings where inference fails, annotate: `let e: Events<_, 1> = events![A, B];`
- `add()` remains public but delegates to `ArrayVec::try_push` with a clear panic message on overflow. The Handle's N declaration prevents this at the type level.
- `ArrayVec<T, 0>` is valid (verified) — 4 bytes overhead for the length counter.

**Blast radius:** All changes are within `crates/nexus/`. No store, fjall, or example code references `SmallVec` or the internal fields of `Events`. The `apply_events` method gains a const generic parameter but is only called within kernel tests.

---

### Task 1: Swap workspace dependency from smallvec to arrayvec

**Files:**
- Modify: `Cargo.toml` (workspace root, lines 23)
- Modify: `crates/nexus/Cargo.toml` (line 15)

**Step 1: Update workspace root Cargo.toml**

Replace the smallvec workspace dependency with arrayvec:

```toml
# Remove:
smallvec = "1.15.1"
# Add:
arrayvec = { version = "0.7.6", default-features = false }
```

**Step 2: Update nexus crate Cargo.toml**

```toml
# Remove:
smallvec = { workspace = true }
# Add:
arrayvec = { workspace = true }
```

**Step 3: Verify it compiles (expect errors — Events not yet updated)**

Run: `cargo check -p nexus 2>&1 | head -5`
Expected: errors about `smallvec` not found in events.rs — confirms dependency swap worked.

**Step 4: Run `cargo hakari generate` if workspace-hack needs updating**

Run: `cargo hakari generate && cargo hakari manage-deps`

---

### Task 2: Rewrite Events<E, N> with ArrayVec

**Files:**
- Modify: `crates/nexus/src/events.rs` (full rewrite)

**Step 1: Write the failing test (compile check)**

The existing tests in `crates/nexus/tests/kernel_tests/events_tests.rs` will fail to compile after this change. That's the failing test — no new test needed yet.

**Step 2: Rewrite events.rs**

Replace the entire contents of `crates/nexus/src/events.rs` with:

```rust
use arrayvec::{ArrayVec, IntoIter as ArrayVecIntoIter};

use crate::event::DomainEvent;

use core::iter::{Chain, Once, once};

/// A non-empty collection of domain events with compile-time capacity.
///
/// `Events<E, N>` guarantees at least one event (`first`) and can hold
/// up to `N` additional events in a stack-allocated `ArrayVec`. Total
/// capacity is `N + 1`.
///
/// - `N = 0` (default): exactly one event — the common case for most commands.
/// - `N = 2`: up to three events.
///
/// No heap allocation. `no_std` + `no_alloc` compatible.
///
/// Construct via the [`events!`] macro or [`Events::new`].
#[derive(Debug)]
pub struct Events<E: DomainEvent, const N: usize = 0> {
    first: E,
    rest: ArrayVec<E, N>,
}

impl<E: DomainEvent, const N: usize> Events<E, N> {
    #[must_use]
    pub fn new(event: E) -> Self {
        Self {
            first: event,
            rest: ArrayVec::new(),
        }
    }

    /// Add an event to the collection.
    ///
    /// # Panics
    ///
    /// Panics if the collection is at capacity (`N` additional events
    /// already stored). This indicates a programming error — the
    /// `Handle` trait's `N` parameter should match the maximum number
    /// of additional events the handler produces.
    pub fn add(&mut self, event: E) {
        #[allow(clippy::expect_used, reason = "capacity overflow is a programmer bug, enforced by Handle<C, N>")]
        self.rest.try_push(event).expect(
            "Events capacity exceeded: the Handle trait's N parameter must match the maximum number of additional events",
        );
    }

    /// Returns an iterator over references to the events.
    pub fn iter(&self) -> Chain<Once<&E>, core::slice::Iter<'_, E>> {
        once(&self.first).chain(self.rest.iter())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rest.len() + 1
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl<E: DomainEvent + Clone, const N: usize> Clone for Events<E, N> {
    fn clone(&self) -> Self {
        Self {
            first: self.first.clone(),
            rest: self.rest.clone(),
        }
    }
}

impl<E: DomainEvent + PartialEq, const N: usize> PartialEq for Events<E, N> {
    fn eq(&self, other: &Self) -> bool {
        self.first == other.first && self.rest == other.rest
    }
}

impl<E: DomainEvent + Eq, const N: usize> Eq for Events<E, N> {}

impl<E: DomainEvent, const N: usize> From<E> for Events<E, N> {
    fn from(event: E) -> Self {
        Self::new(event)
    }
}

impl<E: DomainEvent, const N: usize> IntoIterator for Events<E, N> {
    type Item = E;
    type IntoIter = Chain<Once<E>, ArrayVecIntoIter<E, N>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.first).chain(self.rest)
    }
}

impl<'a, E: DomainEvent, const N: usize> IntoIterator for &'a Events<E, N> {
    type Item = &'a E;
    type IntoIter = Chain<Once<&'a E>, core::slice::Iter<'a, E>>;

    fn into_iter(self) -> Self::IntoIter {
        once(&self.first).chain(self.rest.iter())
    }
}
```

**Step 3: Update the events! macro** (same file, prepend before the struct)

```rust
#[macro_export]
macro_rules! events {
    [$head:expr] => {
        $crate::Events::new($head)
    };
    [$head:expr, $($tail:expr),+ $(,)?] => {
        {
            let mut events = $crate::Events::new($head);
            $(
                events.add($tail);
            )*
            events
        }
    };
}
```

The macro is **unchanged** — it already works with type inference. `N` is inferred from the return type in Handle impls. In test let-bindings, add a type annotation.

**Step 4: Verify events.rs compiles**

Run: `cargo check -p nexus 2>&1 | head -20`
Expected: events.rs compiles. Errors may come from other files (aggregate.rs, tests) — those are next tasks.

---

### Task 3: Update aggregate.rs — make apply_events generic over N

**Files:**
- Modify: `crates/nexus/src/aggregate.rs` (line 370)

**Step 1: Update apply_events signature**

Change:
```rust
pub fn apply_events(&mut self, events: &Events<EventOf<A>>) {
```
To:
```rust
pub fn apply_events<const N: usize>(&mut self, events: &Events<EventOf<A>, N>) {
```

The body is unchanged — it just iterates.

**Step 2: Verify aggregate.rs compiles**

Run: `cargo check -p nexus 2>&1 | head -20`
Expected: compiles (or only test errors remain).

---

### Task 4: Update Handle trait with const generic N

**Files:**
- Modify: `crates/nexus/src/aggregate.rs` (line 163)

**Step 1: Update Handle trait**

Change:
```rust
pub trait Handle<C>: AggregateEntity {
    fn handle(&self, cmd: C) -> Result<Events<EventOf<Self>>, <Self as Aggregate>::Error>;
}
```
To:
```rust
pub trait Handle<C, const N: usize = 0>: AggregateEntity {
    fn handle(&self, cmd: C) -> Result<Events<EventOf<Self>, N>, <Self as Aggregate>::Error>;
}
```

**Step 2: Update Handle documentation example**

Update the doc example to show the default (N=0) case and a multi-event case.

**Step 3: Verify it compiles**

Run: `cargo check -p nexus 2>&1 | head -20`
Expected: kernel source compiles. Test compilation errors expected.

---

### Task 5: Update kernel tests — aggregate_root_tests.rs

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/aggregate_root_tests.rs`

**Step 1: Update Handle impls**

The existing `Handle<Increment>`, `Handle<IncrementBy>`, `Handle<Decrement>` impls all return single events, so they keep default `N=0`. No change needed to the impl signatures — the default works.

**Step 2: Update let-binding type annotations where inference fails**

Any `let decided = events![...]` that doesn't flow into a typed context needs annotation. Specifically:

```rust
// Line 362 — 2 events, needs N=1
let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::Incremented];

// Line 375 — 2 events, needs N=1
let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::IncrementedBy(9)];

// Line 393 — 2 events, needs N=1
let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::Incremented];
```

**Step 3: Run tests**

Run: `cargo test -p nexus -- aggregate_root_tests`
Expected: all pass.

---

### Task 6: Update kernel tests — events_tests.rs

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/events_tests.rs`

**Step 1: Add type annotations where needed**

```rust
// events_add_increases_len — Events with add needs N >= 1
let mut events: Events<_, 1> = Events::new(TestEvent::Created(Created));

// events_into_iter — same
let mut events: Events<_, 1> = Events::new(TestEvent::Created(Created));

// events_macro_multiple — 2 events, N=1
let events: Events<_, 1> = nexus::events![TestEvent::Created(Created), TestEvent::Activated(Activated)];
```

Single-event tests (`events_guarantees_non_empty`, `events_from_single`, `events_macro_single`) should work without annotation since `Events<E, 0>` is the default.

**Step 2: Run tests**

Run: `cargo test -p nexus -- events_tests`
Expected: all pass.

---

### Task 7: Update remaining kernel test files

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/edge_case_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/integration_test.rs`
- Modify: `crates/nexus/tests/kernel_tests/newtype_aggregate_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/replay_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/security_tests.rs`
- Modify: `crates/nexus/tests/kernel_tests/property_tests.rs`

**Step 1: For each file, find `events!` usage and add type annotations**

Pattern: any `let x = events![a, b, ...]` with 2+ events needs `let x: Events<_, {count-1}> = ...`.
Single-event `events![a]` should work as-is (N=0 default).

Handle impls that return single events keep default N=0.
Handle impls returning multiple events need `Handle<Cmd, N>` with appropriate N.

**Step 2: Run all kernel tests**

Run: `cargo test -p nexus`
Expected: all pass.

---

### Task 8: Update architecture test

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/architecture_tests.rs`

**Step 1: Replace smallvec assertion with arrayvec**

Change:
```rust
assert!(
    cargo_toml.contains("smallvec"),
    "nexus should depend on smallvec"
);
```
To:
```rust
assert!(
    cargo_toml.contains("arrayvec"),
    "nexus should depend on arrayvec"
);
```

**Step 2: Run architecture test**

Run: `cargo test -p nexus -- architecture_tests`
Expected: pass.

---

### Task 9: Update benchmarks

**Files:**
- Modify: `crates/nexus/benches/kernel_bench.rs`

**Step 1: Add type annotations to any events! usage with multiple events**

Same pattern as test files.

**Step 2: Verify benchmarks compile**

Run: `cargo check -p nexus --benches`
Expected: compiles.

---

### Task 10: Update macros tests (if they reference Events)

**Files:**
- Check: `crates/nexus-macros/tests/aggregate_derive.rs`
- Check: `crates/nexus-macros/tests/cross_crate_test/src/lib.rs`
- Check: `crates/nexus-macros/tests/expand_roundtrip.rs`
- Check: `crates/nexus-macros/tests/macro_property_tests.rs`

**Step 1: Grep for events! or Events usage and add annotations**

Same pattern as kernel tests.

**Step 2: Run macros tests**

Run: `cargo test -p nexus-macros`
Expected: tests that were passing before still pass (some may already be failing from prior refactoring).

---

### Task 11: Update store and example code that uses Events

**Files:**
- Check: `crates/nexus-store/tests/` (grep for `events!` or `Events`)
- Check: `examples/inmemory/src/main.rs`
- Check: `examples/store-and-kernel/src/main.rs`
- Check: `examples/store-inmemory/src/main.rs`

**Step 1: Add type annotations to events! usage with multiple events**

Same pattern. Handle impls in examples returning single events keep default N=0.

**Step 2: Run full workspace tests**

Run: `cargo test --all`
Expected: all previously-passing tests pass.

---

### Task 12: Update CLAUDE.md architecture documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update the events.rs description**

Change the `events.rs` line in the architecture section from:
```
`Events<E>` is a `SmallVec`-backed collection guaranteeing at least one event
```
To:
```
`Events<E, const N: usize = 0>` is an `ArrayVec`-backed collection guaranteeing at least one event with compile-time capacity N+1; constructed via `events![e1, e2]` macro. N=0 (default) = single event. The Handle<C, N> trait enforces max events per command at the type level.
```

**Step 2: Update the Handle trait description**

Add N parameter to Handle description.

---

### Task 13: Format, lint, and final verification

**Step 1: Format**

Run: `cargo fmt --all`

**Step 2: Clippy**

Run: `cargo clippy --all-targets -- --deny warnings`
Expected: no warnings.

**Step 3: Full test suite**

Run: `cargo test --all`
Expected: all pass.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat(nexus): replace SmallVec with ArrayVec const-generic Events<E, N>

Events<E, const N: usize = 0> now uses ArrayVec<E, N> for zero-heap,
no_std/no_alloc event storage. Handle<C, const N: usize = 0> enforces
max event count per command handler at the type level.

- N=0 (default): single-event handlers need no annotation
- N=K: handler can return up to K+1 events on the stack
- Drops smallvec dependency, adds arrayvec (no_std, no alloc)

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```
