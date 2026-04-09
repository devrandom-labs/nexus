# Repository Builder Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `Store::repository(codec, upcaster)` and `Store::zero_copy_repository(codec, upcaster)` with a typestate builder that defaults to `JsonCodec` + `()` when features are enabled.

**Architecture:** `RepositoryBuilder<S, C, U>` lives in `store/builder.rs`. `NeedsCodec` is a `!Send` ZST that prevents `build()` without a codec. `Store::repository()` returns the builder with feature-gated defaults. `build()` and `build_zero_copy()` are terminal methods.

**Tech Stack:** Rust, PhantomData for `!Send` trick, `#[cfg]` for feature-gated defaults

---

### Task 1: Create `RepositoryBuilder` and `NeedsCodec`

**Files:**
- Create: `crates/nexus-store/src/store/builder.rs`
- Modify: `crates/nexus-store/src/store/mod.rs`
- Modify: `crates/nexus-store/src/store/store.rs`
- Modify: `crates/nexus-store/src/lib.rs`

**Step 1: Create `crates/nexus-store/src/store/builder.rs`**

```rust
use std::marker::PhantomData;

use super::event_store::EventStore;
use super::store::Store;
use super::zero_copy_event_store::ZeroCopyEventStore;

/// Placeholder indicating no codec has been configured yet.
///
/// Call [`.codec()`](RepositoryBuilder::codec) on the builder to set one,
/// or enable the `json` feature for an automatic [`JsonCodec`](crate::JsonCodec)
/// default.
///
/// `NeedsCodec` is intentionally `!Send`, which prevents calling
/// [`build()`](RepositoryBuilder::build) before a codec is configured
/// (since `build()` requires `C: Send + Sync + 'static`).
pub struct NeedsCodec(PhantomData<*const ()>);

/// Builder for constructing [`EventStore`] or [`ZeroCopyEventStore`] facades.
///
/// Created via [`Store::repository()`](Store::repository). Configure the
/// codec and upcaster, then call [`build()`](Self::build) or
/// [`build_zero_copy()`](Self::build_zero_copy).
///
/// # Default codec
///
/// - With the `json` feature: defaults to [`JsonCodec`](crate::JsonCodec).
/// - Without a codec feature: starts as [`NeedsCodec`] — you must call
///   [`.codec()`](Self::codec) before building.
///
/// # Default upcaster
///
/// Always defaults to `()` (no-op passthrough).
///
/// # Examples
///
/// ```ignore
/// // Zero-config (json feature):
/// let repo = store.repository().build();
///
/// // Custom codec:
/// let repo = store.repository().codec(MyCodec).build();
///
/// // Custom codec + upcaster:
/// let repo = store.repository()
///     .codec(MyCodec)
///     .upcaster(MyTransforms)
///     .build();
/// ```
pub struct RepositoryBuilder<S, C, U> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}

impl<S, C, U> RepositoryBuilder<S, C, U> {
    /// Set the codec for event serialization/deserialization.
    ///
    /// Overrides any feature-provided default.
    pub fn codec<NewC>(self, codec: NewC) -> RepositoryBuilder<S, NewC, U> {
        RepositoryBuilder {
            store: self.store,
            codec,
            upcaster: self.upcaster,
        }
    }

    /// Set the upcaster for schema evolution.
    ///
    /// Defaults to `()` (no-op) if not called.
    pub fn upcaster<NewU>(self, upcaster: NewU) -> RepositoryBuilder<S, C, NewU> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster,
        }
    }
}

impl<S, C: Send + Sync + 'static, U> RepositoryBuilder<S, C, U> {
    /// Build an [`EventStore`] using an owning [`Codec`](crate::Codec).
    ///
    /// For serde-based formats (JSON, bincode, postcard) where
    /// deserialization produces an owned value.
    pub fn build(self) -> EventStore<S, C, U> {
        EventStore::new(self.store, self.codec, self.upcaster)
    }

    /// Build a [`ZeroCopyEventStore`] using a [`BorrowingCodec`](crate::BorrowingCodec).
    ///
    /// For zero-copy formats (rkyv, flatbuffers) where the serialized
    /// bytes are reinterpreted in-place.
    pub fn build_zero_copy(self) -> ZeroCopyEventStore<S, C, U> {
        ZeroCopyEventStore::new(self.store, self.codec, self.upcaster)
    }
}

// -- Store entry points --

impl<S> Store<S> {
    /// Start building a repository backed by this store.
    ///
    /// With the `json` feature enabled, the codec defaults to
    /// [`JsonCodec`](crate::JsonCodec) — call [`.build()`](RepositoryBuilder::build)
    /// directly for zero-config usage.
    ///
    /// Without a codec feature, call [`.codec()`](RepositoryBuilder::codec)
    /// before building.
    #[cfg(feature = "json")]
    pub fn repository(&self) -> RepositoryBuilder<S, crate::JsonCodec, ()> {
        RepositoryBuilder {
            store: self.clone(),
            codec: crate::JsonCodec::default(),
            upcaster: (),
        }
    }

    /// Start building a repository backed by this store.
    ///
    /// No codec feature is enabled — you must call
    /// [`.codec()`](RepositoryBuilder::codec) before
    /// [`.build()`](RepositoryBuilder::build).
    #[cfg(not(any(feature = "json")))]
    pub fn repository(&self) -> RepositoryBuilder<S, NeedsCodec, ()> {
        RepositoryBuilder {
            store: self.clone(),
            codec: NeedsCodec(PhantomData),
            upcaster: (),
        }
    }
}
```

**Step 2: Wire up module in `crates/nexus-store/src/store/mod.rs`**

Add `mod builder;` and `pub use builder::{NeedsCodec, RepositoryBuilder};`

**Step 3: Remove old methods from `crates/nexus-store/src/store/store.rs`**

Delete the `impl<S: RawEventStore> Store<S>` block containing `repository()` and `zero_copy_repository()`. The `repository()` entry point now lives in `builder.rs`. Update the doc comment on `Store` to reference the new builder API.

**Step 4: Add crate-level re-export in `crates/nexus-store/src/lib.rs`**

Add `RepositoryBuilder` to the `pub use store::` line.

**Step 5: Verify compilation**

Run: `cargo check -p nexus-store --features json`
Run: `cargo check -p nexus-store`
Run: `cargo clippy -p nexus-store --features json -- --deny warnings`

---

### Task 2: Migrate nexus-store tests to builder API

**Files:**
- Modify: `crates/nexus-store/tests/event_store_tests.rs`
- Modify: `crates/nexus-store/tests/zero_copy_event_store_tests.rs`
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/serde_codec_tests.rs`

**Migration pattern:**

Old:
```rust
let es = store.repository(TestCodec, ());
let es = store.repository(TestCodec, SomeUpcaster);
let es = store.zero_copy_repository(BorrowCodec, ());
```

New:
```rust
let es = store.repository().codec(TestCodec).build();
let es = store.repository().codec(TestCodec).upcaster(SomeUpcaster).build();
let es = store.repository().codec(BorrowCodec).build_zero_copy();
```

Special case — `serde_codec_tests.rs` (has `json` feature):
```rust
// Old:
let repo = store.repository(JsonCodec::default(), ());
// New:
let repo = store.repository().build();
```

**Step 1: Migrate all call sites**

Apply the mechanical transform to every test file. There are approximately:
- `event_store_tests.rs`: 7 call sites
- `zero_copy_event_store_tests.rs`: 3 call sites
- `repository_qa_tests.rs`: ~40 call sites
- `adversarial_property_tests.rs`: ~11 call sites
- `serde_codec_tests.rs`: 1 call site

**Step 2: Run all tests**

Run: `cargo test -p nexus-store --features "json,testing"` — all tests must pass.
Run: `cargo test -p nexus-store --features testing` — all tests without json must pass.
Run: `cargo clippy -p nexus-store --all-features --tests -- --deny warnings`

---

### Task 3: Update doc comments and examples

**Files:**
- Modify: `crates/nexus-store/src/store/event_store.rs` (doc comment)
- Modify: `crates/nexus-store/src/store/zero_copy_event_store.rs` (doc comment)

**Step 1: Update EventStore doc comments**

Replace the `# Construction` examples:
```rust
/// # Construction
///
/// Created via [`Store::repository()`](super::store::Store::repository):
///
/// ```ignore
/// let store = Store::new(backend);
/// let orders = store.repository().codec(OrderCodec).build();
/// let orders = store.repository().codec(OrderCodec).upcaster(OrderTransforms).build();
/// ```
```

**Step 2: Update ZeroCopyEventStore doc comments**

Replace the `# Construction` examples:
```rust
/// # Construction
///
/// Created via [`Store::repository()`](super::store::Store::repository):
///
/// ```ignore
/// let store = Store::new(backend);
/// let orders = store.repository().codec(OrderCodec).build_zero_copy();
/// ```
```

**Step 3: Verify**

Run: `cargo clippy -p nexus-store --all-features --tests -- --deny warnings`
Run: `cargo test --all`

---

### Task 4: Full verification and commit

**Step 1: Format**

Run: `cargo fmt --all`

**Step 2: Clippy all feature combos**

Run: `cargo clippy -p nexus-store -- --deny warnings`
Run: `cargo clippy -p nexus-store --features serde -- --deny warnings`
Run: `cargo clippy -p nexus-store --features json -- --deny warnings`
Run: `cargo clippy -p nexus-store --all-features --tests -- --deny warnings`

**Step 3: Full workspace test**

Run: `cargo test --all`

**Step 4: Hakari check**

Run: `cargo hakari generate --diff`

**Step 5: Commit**

```bash
git add crates/nexus-store/src/store/builder.rs \
       crates/nexus-store/src/store/store.rs \
       crates/nexus-store/src/store/mod.rs \
       crates/nexus-store/src/store/event_store.rs \
       crates/nexus-store/src/store/zero_copy_event_store.rs \
       crates/nexus-store/src/lib.rs \
       crates/nexus-store/tests/ \
       docs/plans/
git commit -m "feat(store)!: replace repository() with typestate builder API"
```
