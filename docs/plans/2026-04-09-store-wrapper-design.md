# Store Wrapper & EventStore Segregation

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce `Store<S>` as an `Arc`-wrapped handle to any `RawEventStore`, with `repository()` / `zero_copy_repository()` factory methods, and split `EventStore` / `ZeroCopyEventStore` into their own files.

**Architecture:** `Store<S>` is a newtype over `Arc<S>` — cheap to clone, no generics beyond the backend. `EventStore<S, C, U>` and `ZeroCopyEventStore<S, C, U>` hold `Store<S>` instead of bare `S`. Users create one `Store`, then call `.repository(codec, upcaster)` per aggregate to get typed facades. The `RawEventStore` trait and its impls are untouched.

**Tech Stack:** Rust, nexus-store crate. No new dependencies.

---

### Task 1: Create `store/store.rs` — the `Store<S>` wrapper

**Files:**
- Create: `crates/nexus-store/src/store/store.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`

**Step 1: Create `store.rs` with `Store<S>`**

```rust
use std::sync::Arc;

use super::raw::RawEventStore;

/// Shared handle to a [`RawEventStore`] backend.
///
/// `Store` wraps the backend in an `Arc`, making it cheap to clone and
/// safe to share across tasks. It carries no codec, upcaster, or
/// aggregate binding — it is just a database handle.
///
/// Use [`repository()`](Store::repository) or
/// [`zero_copy_repository()`](Store::zero_copy_repository) to create
/// typed facades bound to a specific aggregate's codec and upcaster.
///
/// # Example
///
/// ```ignore
/// let store = Store::new(FjallStore::builder("path").open()?);
///
/// let orders = store.repository(OrderCodec, OrderUpcaster);
/// let users  = store.repository(UserCodec, ());
/// ```
#[derive(Debug)]
pub struct Store<S> {
    inner: Arc<S>,
}

impl<S> Store<S> {
    /// Wrap a raw event store backend in a shared handle.
    pub fn new(raw: S) -> Self {
        Self {
            inner: Arc::new(raw),
        }
    }
}

impl<S> Clone for Store<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: RawEventStore> Store<S> {
    /// Create a typed event store (owning codec) for a specific aggregate.
    pub fn repository<C, U>(
        &self,
        codec: C,
        upcaster: U,
    ) -> super::event_store::EventStore<S, C, U> {
        super::event_store::EventStore::new(self.clone(), codec, upcaster)
    }

    /// Create a typed event store (borrowing codec) for a specific aggregate.
    pub fn zero_copy_repository<C, U>(
        &self,
        codec: C,
        upcaster: U,
    ) -> super::zero_copy_event_store::ZeroCopyEventStore<S, C, U> {
        super::zero_copy_event_store::ZeroCopyEventStore::new(self.clone(), codec, upcaster)
    }
}
```

**Step 2: Add `Store` to re-exports in `mod.rs`**

`mod.rs` becomes:
```rust
mod event_store;
mod raw;
mod repository;
mod store;
mod stream;
mod zero_copy_event_store;

pub use event_store::EventStore;
pub use raw::RawEventStore;
pub use repository::Repository;
pub use store::Store;
pub use stream::EventStream;
pub use zero_copy_event_store::ZeroCopyEventStore;
```

**Step 3: Add `Store` to crate-level re-exports in `lib.rs`**

Add `Store` to the `pub use store::` line.

**Step 4: Run `cargo check -p nexus-store`**

Expected: compiles (no consumers yet).

**Step 5: Commit**

```
feat(store): add Store<S> wrapper with Arc-shared backend
```

---

### Task 2: Split `ZeroCopyEventStore` into its own file

**Files:**
- Create: `crates/nexus-store/src/store/zero_copy_event_store.rs`
- Modify: `crates/nexus-store/src/store/event_store.rs` (remove `ZeroCopyEventStore`)
- Modify: `crates/nexus-store/src/store/mod.rs` (update re-exports)

**Step 1: Move `ZeroCopyEventStore` and its impls out of `event_store.rs` into `zero_copy_event_store.rs`**

Cut everything from the `// ZeroCopyEventStore` section comment to the end of the file. Place it in the new file with the necessary imports.

**Step 2: Update `mod.rs`**

Already done in Task 1 (the `mod.rs` listed there includes `mod zero_copy_event_store`).

**Step 3: Run `cargo check -p nexus-store`**

Expected: compiles.

**Step 4: Commit**

```
refactor(store): split ZeroCopyEventStore into its own module
```

---

### Task 3: Refactor `EventStore` to hold `Store<S>` instead of bare `S`

**Files:**
- Modify: `crates/nexus-store/src/store/event_store.rs`

**Step 1: Change the struct and constructors**

```rust
use super::store::Store;

pub struct EventStore<S, C, U = ()> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}

impl<S, C, U> EventStore<S, C, U> {
    /// Create an event store bound to a shared store, codec, and upcaster.
    pub(crate) const fn new(store: Store<S>, codec: C, upcaster: U) -> Self {
        Self { store, codec, upcaster }
    }
}
```

Remove the old `EventStore::new(store, codec)` and `EventStore::with_upcaster(store, codec, upcaster)` constructors. The only construction path is now `Store::repository()`.

**Step 2: Update `Repository` impl to deref through `Store`**

In the `Repository<A> for EventStore<S, C, U>` impl, change `self.store.read_stream(...)` and `self.store.append(...)` to `self.store.inner.read_stream(...)` and `self.store.inner.append(...)`.

BUT — `inner` is private. So add a `pub(crate)` accessor to `Store`:

```rust
impl<S> Store<S> {
    pub(crate) fn raw(&self) -> &S {
        &self.inner
    }
}
```

Then use `self.store.raw().read_stream(...)` and `self.store.raw().append(...)`.

**Step 3: Run `cargo check -p nexus-store`**

Expected: compilation errors in tests (they still use the old constructors). That's Task 5.

**Step 4: Commit**

```
refactor(store): EventStore holds Store<S> instead of bare S
```

---

### Task 4: Refactor `ZeroCopyEventStore` to hold `Store<S>` instead of bare `S`

**Files:**
- Modify: `crates/nexus-store/src/store/zero_copy_event_store.rs`

Same pattern as Task 3. Change struct to hold `Store<S>`, remove old constructors, use `self.store.raw()` for backend access.

**Step 1: Apply same changes as Task 3**

**Step 2: Run `cargo check -p nexus-store`**

Expected: compilation errors in tests only (old constructors).

**Step 3: Commit**

```
refactor(store): ZeroCopyEventStore holds Store<S> instead of bare S
```

---

### Task 5: Migrate all test call sites

**Files:**
- Modify: `crates/nexus-store/tests/event_store_tests.rs`
- Modify: `crates/nexus-store/tests/zero_copy_event_store_tests.rs`
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/property_tests.rs`
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs`
- Modify: `crates/nexus-store/tests/security_tests.rs`
- Modify: `crates/nexus-store/tests/integration_tests.rs`
- Modify: `crates/nexus-store/tests/inmemory_store_tests.rs`
- Modify: `crates/nexus-store/benches/store_bench.rs`

**Migration pattern:**

Old:
```rust
let es = EventStore::new(InMemoryStore::new(), TestCodec);
let es = EventStore::with_upcaster(InMemoryStore::new(), TestCodec, SomeUpcaster);
let es = Arc::new(EventStore::new(InMemoryStore::new(), SimpleCodec));
```

New:
```rust
let store = Store::new(InMemoryStore::new());
let es = store.repository(TestCodec, ());
let es = store.repository(TestCodec, SomeUpcaster);
let es = store.repository(SimpleCodec, ());  // Store is already Arc-wrapped, no Arc::new needed
```

For concurrency tests that used `Arc::new(EventStore::new(...))`: the `EventStore` now contains `Store` which is `Arc`-wrapped, but `EventStore` itself still needs to be shared. However — `Store` is `Clone` (cheap arc bump), so `EventStore` can derive `Clone` if codec and upcaster are `Clone`. If they're not, the test can wrap in `Arc` as before. Keep `Arc<EventStore>` for concurrency tests — it works, and `EventStore` shouldn't be forced to be `Clone`.

**Step 1: Migrate all test files mechanically**

Apply the pattern above across all files. Add `use nexus_store::Store;` or `use nexus_store::store::Store;` to imports.

**Step 2: Run `cargo test -p nexus-store`**

Expected: all tests pass.

**Step 3: Run `cargo clippy -p nexus-store -- --deny warnings`**

Expected: clean.

**Step 4: Commit**

```
refactor(store): migrate all tests to Store::repository() API
```

---

### Task 6: Update `lib.rs` re-exports and doc comments

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`
- Modify: `crates/nexus-store/src/store/event_store.rs` (doc comments)
- Modify: `crates/nexus-store/src/store/zero_copy_event_store.rs` (doc comments)

**Step 1: Update `lib.rs` re-exports**

Ensure `Store` is exported:
```rust
pub use store::{EventStore, EventStream, RawEventStore, Repository, Store, ZeroCopyEventStore};
```

**Step 2: Update doc comments on `EventStore` and `ZeroCopyEventStore`**

Replace the `# Construction` examples in their doc comments:

Old:
```rust
/// let es = EventStore::new(store, codec);
/// let es = EventStore::with_upcaster(store, codec, OrderTransforms);
```

New:
```rust
/// let store = Store::new(backend);
/// let es = store.repository(codec, ());
/// let es = store.repository(codec, OrderTransforms);
```

**Step 3: Run `cargo test --all && cargo clippy --all-targets -- --deny warnings && cargo fmt --all --check`**

Expected: all green.

**Step 4: Commit**

```
docs(store): update construction examples for Store wrapper API
```

---

### Task 7: Update CLAUDE.md architecture section

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add `Store` to the Store Crate section**

Add to `store/` listing:
```
- `store.rs` — `Store<S>`: `Arc`-wrapped shared handle to any `RawEventStore`. Clone-cheap. Factory methods: `repository(codec, upcaster) -> EventStore`, `zero_copy_repository(codec, upcaster) -> ZeroCopyEventStore`.
```

Update `event_store.rs` description to mention it holds `Store<S>` not bare `S`.

**Step 2: Commit**

```
docs: update CLAUDE.md architecture for Store wrapper
```
