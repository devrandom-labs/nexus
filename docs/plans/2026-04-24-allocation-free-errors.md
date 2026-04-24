# Allocation-Free Error Paths Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the entire error path in `nexus-store` and `nexus-fjall` allocation-free by replacing `Box<dyn Error>`, `String`, and `StreamLabel` with generic type parameters and `ArrayString<64>`.

**Architecture:** `StoreError` becomes `StoreError<A, C, U>` generic over adapter/codec/upcaster errors. `UpcastError` becomes `UpcastError<U>` generic over transform errors. `StreamLabel` is deleted and replaced by `Id::to_label() -> ArrayString<64>` (default method on kernel trait). `FjallError` replaces `String` fields with `ArrayString`.

**Tech Stack:** `arrayvec::ArrayString<64>`, `std::convert::Infallible`, existing `arrayvec` workspace dependency.

---

### Task 1: Add `Id::to_label()` default method to kernel

**Files:**
- Modify: `crates/nexus/src/id.rs`
- Test: `crates/nexus/tests/kernel_tests/traits_tests.rs`

**Step 1: Write the failing test**

In `crates/nexus/tests/kernel_tests/traits_tests.rs`, add:

```rust
#[test]
fn id_to_label_returns_display_as_array_string() {
    let id = TestId("hello".into());
    let label = id.to_label();
    assert_eq!(label.as_str(), "hello");
}

#[test]
fn id_to_label_truncates_long_ids() {
    let long = "a".repeat(100);
    let id = TestId(long);
    let label = id.to_label();
    assert!(label.len() <= 64);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus -- traits_tests::id_to_label`
Expected: FAIL — `to_label` method does not exist.

**Step 3: Write minimal implementation**

In `crates/nexus/src/id.rs`:

```rust
use arrayvec::ArrayString;
use std::fmt::{Debug, Display, Write};
use std::hash::Hash;

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {
    const BYTE_LEN: usize;

    /// Stack-allocated diagnostic label for error messages.
    /// Truncates at 64 bytes if the display representation is longer.
    fn to_label(&self) -> ArrayString<64> {
        let mut buf = ArrayString::<64>::new();
        let _ = write!(buf, "{self}");
        buf
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus -- traits_tests::id_to_label`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus/src/id.rs crates/nexus/tests/kernel_tests/traits_tests.rs
git commit -m "feat(kernel): add Id::to_label() default method returning ArrayString<64>"
```

---

### Task 2: Make `UpcastError` generic over transform error `U`

**Files:**
- Modify: `crates/nexus-store/src/error.rs` (UpcastError definition)
- Modify: `crates/nexus-store/src/lib.rs` (re-export)
- Test: `crates/nexus-store/tests/error_tests.rs`

**Step 1: Write the failing test**

In `crates/nexus-store/tests/error_tests.rs`, add:

```rust
#[test]
fn upcast_error_with_concrete_transform_error() {
    #[derive(Debug)]
    struct MyTransformError;
    impl std::fmt::Display for MyTransformError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "transform failed")
        }
    }
    impl std::error::Error for MyTransformError {}

    let err = UpcastError::<MyTransformError>::TransformFailed {
        event_type: ArrayString::from("AccountOpened").unwrap(),
        schema_version: Version::INITIAL,
        source: MyTransformError,
    };
    let msg = format!("{err}");
    assert!(msg.contains("AccountOpened"));
    assert!(msg.contains("transform failed"));
}

#[test]
fn upcast_error_infallible_transform_failed_is_uninhabitable() {
    // UpcastError<Infallible>::TransformFailed cannot be constructed.
    // This test just verifies the other variants work with Infallible.
    let err = UpcastError::<std::convert::Infallible>::VersionNotAdvanced {
        event_type: ArrayString::from("Foo").unwrap(),
        input_version: Version::INITIAL,
        output_version: Version::INITIAL,
    };
    assert!(format!("{err}").contains("Foo"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- error_tests::upcast_error`
Expected: FAIL — `UpcastError` is not generic.

**Step 3: Write minimal implementation**

In `crates/nexus-store/src/error.rs`, replace the `UpcastError` definition:

```rust
use arrayvec::ArrayString;

#[derive(Debug)]
#[non_exhaustive]
pub enum UpcastError<U> {
    VersionNotAdvanced {
        event_type: ArrayString<64>,
        input_version: Version,
        output_version: Version,
    },
    EmptyEventType {
        input_event_type: ArrayString<64>,
        schema_version: Version,
    },
    ChainLimitExceeded {
        event_type: ArrayString<64>,
        schema_version: Version,
        limit: u64,
    },
    TransformFailed {
        event_type: ArrayString<64>,
        schema_version: Version,
        source: U,
    },
}
```

Implement `Display` and `Error` manually (since `thiserror` may struggle with the generic + `ArrayString` combo — verify and use `thiserror` if it works, otherwise hand-impl).

Update `crates/nexus-store/src/lib.rs` re-export to include the generic.

**Step 4: Fix all compilation errors**

Files that reference `UpcastError` without a type parameter:
- `crates/nexus-store/src/upcasting/upcaster.rs` — `Upcaster` trait (Task 3)
- `crates/nexus-store/tests/adversarial_property_tests.rs` — test constructions
- `crates/nexus-store/tests/error_tests.rs` — existing tests
- `crates/nexus-macros/src/lib.rs` — proc macro codegen (Task 7)

For now, temporarily use a concrete placeholder where needed to keep compiling. The proc macro and Upcaster trait changes come in later tasks.

**Step 5: Run tests to verify they pass**

Run: `cargo test -p nexus-store -- error_tests`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/nexus-store/src/error.rs crates/nexus-store/src/lib.rs crates/nexus-store/tests/error_tests.rs
git commit -m "refactor(store)!: make UpcastError<U> generic over transform error"
```

---

### Task 3: Add associated `Error` type to `Upcaster` trait

**Files:**
- Modify: `crates/nexus-store/src/upcasting/upcaster.rs`
- Test: `crates/nexus-store/tests/property_tests.rs` (existing upcaster tests)

**Step 1: Write the failing test**

In `crates/nexus-store/tests/property_tests.rs` or a new test file, add:

```rust
#[test]
fn unit_upcaster_error_is_infallible() {
    fn assert_infallible<U: Upcaster<Error = std::convert::Infallible>>() {}
    assert_infallible::<()>();
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- unit_upcaster_error_is_infallible`
Expected: FAIL — `Upcaster` has no associated `Error` type.

**Step 3: Write minimal implementation**

In `crates/nexus-store/src/upcasting/upcaster.rs`:

```rust
use super::morsel::EventMorsel;
use crate::error::UpcastError;
use nexus::Version;

pub trait Upcaster: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError<Self::Error>>;
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

impl Upcaster for () {
    type Error = std::convert::Infallible;

    #[inline]
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        Ok(morsel)
    }

    #[inline]
    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}
```

**Step 4: Fix compilation in dependent code**

Update `EventStore` and `ZeroCopyEventStore` to use `U::Error` in their `map_err` calls:
- `crates/nexus-store/src/store/repository/event_store.rs`
- `crates/nexus-store/src/store/repository/zero_copy.rs`

**Step 5: Run tests**

Run: `cargo test -p nexus-store`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/nexus-store/src/upcasting/upcaster.rs
git commit -m "refactor(store)!: add Upcaster::Error associated type, () uses Infallible"
```

---

### Task 4: Make `StoreError<A, C, U>` generic with `VersionOverflow` variant

**Files:**
- Modify: `crates/nexus-store/src/error.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/error_tests.rs`
- Test: `crates/nexus-store/tests/static_assertions.rs`

**Step 1: Write the failing test**

In `crates/nexus-store/tests/error_tests.rs`, add:

```rust
use std::convert::Infallible;

// Concrete error for testing
#[derive(Debug)]
struct TestAdapterError;
impl std::fmt::Display for TestAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "adapter failed")
    }
}
impl std::error::Error for TestAdapterError {}

#[derive(Debug)]
struct TestCodecError;
impl std::fmt::Display for TestCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codec failed")
    }
}
impl std::error::Error for TestCodecError {}

#[test]
fn store_error_version_overflow_is_zero_size() {
    let err: StoreError<TestAdapterError, TestCodecError, Infallible> = StoreError::VersionOverflow;
    assert!(format!("{err}").contains("overflow"));
}

#[test]
fn store_error_conflict_uses_array_string() {
    let err: StoreError<TestAdapterError, TestCodecError, Infallible> = StoreError::Conflict {
        stream_id: ArrayString::from("order-42").unwrap(),
        expected: None,
        actual: None,
    };
    assert!(format!("{err}").contains("order-42"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- error_tests::store_error_version`
Expected: FAIL — `StoreError` is not generic, no `VersionOverflow`.

**Step 3: Write minimal implementation**

In `crates/nexus-store/src/error.rs`, replace `StoreError`:

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError<A, C, U> {
    Conflict {
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    StreamNotFound {
        stream_id: ArrayString<64>,
    },
    Adapter(A),
    Codec(C),
    Upcast(UpcastError<U>),
    Kernel(KernelError),
    VersionOverflow,
}
```

Implement `Display` and `Error` manually. `thiserror` derive may not work with 3 generic params — verify, fall back to manual impl if needed.

Update `AppendError` to use `ArrayString<64>`:

```rust
#[derive(Debug)]
pub enum AppendError<A> {
    Conflict {
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    Store(A),
}
```

**Step 4: Fix compilation across the codebase**

This is the largest ripple. Every file using `StoreError` needs type parameters. Key files:
- `crates/nexus-store/src/store/repository/event_store.rs` — `type Error = StoreError<S::Error, C::Error, U::Error>`
- `crates/nexus-store/src/store/repository/zero_copy.rs` — same
- `crates/nexus-store/src/store/repository/replay.rs` — `ReplayFrom` return type becomes generic
- `crates/nexus-store/src/store/snapshot/snapshotting.rs` — `Snapshotting` error type
- `crates/nexus-store/src/testing.rs` — `InMemoryStore` needs own error type (see Task 5)
- `crates/nexus-store/tests/static_assertions.rs` — needs concrete type params

Replace all `StoreError::Codec("string".into())` version overflow hacks with `StoreError::VersionOverflow`.

**Step 5: Run tests**

Run: `cargo test -p nexus-store`
Expected: PASS (some tests may need adjustment for new generic params)

**Step 6: Commit**

```bash
git add crates/nexus-store/src/
git commit -m "refactor(store)!: make StoreError<A,C,U> generic, add VersionOverflow variant"
```

---

### Task 5: Create `InMemoryStoreError` and fix `InMemoryStore`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`
- Test: `crates/nexus-store/tests/inmemory_store_tests.rs`

**Context:** `InMemoryStore` currently uses `StoreError` as its adapter error type, violating rule §3 ("adapter error types must be distinct from facade error types"). With `StoreError` now generic, `InMemoryStore` needs its own concrete error type.

**Step 1: Define `InMemoryStoreError`**

In `crates/nexus-store/src/testing.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InMemoryStoreError {
    #[error("stored event has version 0 — corrupt data")]
    CorruptVersion,

    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,
}
```

These are the only two errors `InMemoryStore` actually produces.

**Step 2: Update `InMemoryStore` impls**

Change all:
- `type Error = StoreError` → `type Error = InMemoryStoreError`
- `StoreError::Codec("...corrupt data...".into())` → `InMemoryStoreError::CorruptVersion`
- `StoreError::Codec("...version overflow...".into())` → `InMemoryStoreError::VersionOverflow`
- `id.to_stream_label()` → `id.to_label()`

**Step 3: Update `AppendError` construction sites**

Replace `AppendError::Conflict { stream_id: id.to_stream_label(), ... }` with `AppendError::Conflict { stream_id: id.to_label(), ... }`.

**Step 4: Run tests**

Run: `cargo test -p nexus-store`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "refactor(store): introduce InMemoryStoreError, decouple from StoreError"
```

---

### Task 6: Update `nexus-fjall` — allocation-free `FjallError` and `FjallStore`

**Files:**
- Modify: `crates/nexus-fjall/Cargo.toml` (add `arrayvec` dep)
- Modify: `crates/nexus-fjall/src/error.rs`
- Modify: `crates/nexus-fjall/src/store.rs`
- Test: `crates/nexus-fjall/tests/` (existing tests should still pass)

**Step 1: Add `arrayvec` dependency**

In `crates/nexus-fjall/Cargo.toml`, add `arrayvec = { workspace = true }` under `[dependencies]`.

**Step 2: Update `FjallError`**

```rust
use arrayvec::ArrayString;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FjallError {
    #[error("fjall error: {0}")]
    Io(#[from] fjall::Error),

    #[error("corrupt value in stream '{stream_id}' at version {version:?}")]
    CorruptValue {
        stream_id: ArrayString<64>,
        version: Option<u64>,
    },

    #[error("corrupt metadata for stream '{stream_id}'")]
    CorruptMeta { stream_id: ArrayString<64> },

    #[error("invalid input for stream '{stream_id}' at version {version}: {reason}")]
    InvalidInput {
        stream_id: ArrayString<64>,
        version: u64,
        reason: ArrayString<128>,
    },

    #[error("version overflow: cannot advance past u64::MAX")]
    VersionOverflow,
}
```

**Step 3: Update `FjallStore::append` and `FjallStore::read_stream`**

Replace all:
- `id.to_string()` (for error fields) → `id.to_label()`
- `id.to_stream_label()` → `id.to_label()`
- `String` reason fields → `ArrayString::<128>::from("...").unwrap_or(...)`

Note: `id.to_string()` used for the stream metadata key (HashMap key in `streams` partition) stays as-is — that's the storage key path, not the error path. Only error construction sites change.

**Step 4: Run tests**

Run: `cargo test -p nexus-fjall`
Expected: PASS

**Step 5: Run `cargo hakari generate`**

Run: `cargo hakari generate` to update workspace-hack after new dependency.

**Step 6: Commit**

```bash
git add crates/nexus-fjall/
git commit -m "refactor(fjall)!: allocation-free FjallError with ArrayString fields"
```

---

### Task 7: Update proc macro for generic `UpcastError<U>`

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs` (lines ~304-318, ~357-380)

**Step 1: Update codegen for `UpcastError::TransformFailed`**

Change the match arm generation (line ~307):

From:
```rust
.map_err(|e| ::nexus_store::UpcastError::TransformFailed {
    event_type: ::std::string::String::from(#event_type),
    schema_version: ::nexus::Version::new(#from_version).expect("nonzero"),
    source: ::std::boxed::Box::new(e),
})
```

To:
```rust
.map_err(|e| ::nexus_store::UpcastError::TransformFailed {
    event_type: {
        let mut buf = ::arrayvec::ArrayString::<64>::new();
        let _ = ::std::fmt::Write::write_str(&mut buf, #event_type);
        buf
    },
    schema_version: ::nexus::Version::new(#from_version).expect("nonzero"),
    source: e,
})
```

**Step 2: Update the generated `Upcaster` impl**

Change the `impl Upcaster for #struct_ident` block (line ~357):

The generated impl needs `type Error`. The transform functions return `Result<Vec<u8>, E>` where `E` is the transform's error type. The macro needs to infer or require the error type.

**Design decision:** The simplest approach is to require all transform functions on a single upcaster struct to return the same error type, and extract it from the first transform's return type. Alternatively, require an explicit `#[error = MyError]` attribute on the impl block. The explicit attribute is more robust:

```rust
#[nexus::transforms(error = serde_json::Error)]
impl MyTransforms {
    #[transform(event = "AccountOpened", from = 1, to = 2)]
    fn v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, serde_json::Error> { ... }
}
```

Generates:
```rust
impl ::nexus_store::Upcaster for MyTransforms {
    type Error = serde_json::Error;
    // ...
}
```

**Step 3: Update return type in generated code**

Change `::nexus_store::UpcastError` → `::nexus_store::UpcastError<Self::Error>` in the generated impl.

**Step 4: Run macro tests**

Run: `cargo test -p nexus-macros`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-macros/src/lib.rs
git commit -m "refactor(macros)!: update transforms codegen for generic UpcastError<U>"
```

---

### Task 8: Delete `StreamLabel` and `bounded-labels`

**Files:**
- Delete: `crates/nexus-store/src/stream_label.rs`
- Delete: `crates/nexus-store/tests/stream_label_tests.rs`
- Modify: `crates/nexus-store/src/lib.rs` (remove module and re-exports)
- Modify: `crates/nexus-store/Cargo.toml` (remove `bounded-labels` feature)

**Step 1: Remove from `lib.rs`**

Remove:
```rust
pub mod stream_label;
```
and:
```rust
pub use stream_label::{StreamLabel, ToStreamLabel};
```

**Step 2: Delete files**

```bash
rm crates/nexus-store/src/stream_label.rs
rm crates/nexus-store/tests/stream_label_tests.rs
```

**Step 3: Remove `bounded-labels` feature from `Cargo.toml`**

In `crates/nexus-store/Cargo.toml`, remove:
```toml
bounded-labels = []
```

**Step 4: Grep for any remaining references**

Run: `grep -r "StreamLabel\|ToStreamLabel\|to_stream_label\|stream_label\|bounded-labels" crates/`
Fix any remaining references.

**Step 5: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS

**Step 6: Commit**

```bash
git add -A
git commit -m "refactor(store)!: delete StreamLabel, ToStreamLabel, bounded-labels feature"
```

---

### Task 9: Update all remaining tests

**Files:**
- Modify: `crates/nexus-store/tests/error_tests.rs`
- Modify: `crates/nexus-store/tests/static_assertions.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/security_tests.rs`
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs`
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`
- Modify: `crates/nexus-store/tests/snapshot_integration_tests.rs`
- Modify: `crates/nexus-fjall/tests/property_tests.rs`
- Modify: `crates/nexus-fjall/tests/state_machine_tests.rs`
- Modify: `crates/nexus-fjall/tests/resilience_tests.rs`
- Modify: `crates/nexus-store/benches/store_bench.rs`

**Step 1: Fix all test compilation errors**

Update all `StoreError` references to include type parameters. In most test files, define a type alias:

```rust
type TestStoreError = StoreError<InMemoryStoreError, serde_json::Error, std::convert::Infallible>;
```

Or match on the new concrete variants.

Update `AppendError` pattern matches — `stream_id` is now `ArrayString<64>`, not `StreamLabel`.

Update `UpcastError` constructions to use `ArrayString::from(...)` instead of `String::from(...)`.

**Step 2: Fix static assertions**

In `crates/nexus-store/tests/static_assertions.rs`:

```rust
use std::convert::Infallible;
type ConcreteStoreError = StoreError<std::io::Error, std::io::Error, Infallible>;
assert_impl_all!(ConcreteStoreError: std::error::Error, Send, Sync, std::fmt::Debug);
```

**Step 3: Run full workspace tests**

Run: `cargo test --workspace`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/
git commit -m "test: update all tests for generic StoreError<A,C,U>"
```

---

### Task 10: Run `nix flake check` and fix any remaining issues

**Files:** Any that fail clippy, fmt, or tests.

**Step 1: Format**

Run: `cargo fmt --all`

**Step 2: Run full nix flake check**

Run: `nix flake check`
This runs clippy (deny warnings), fmt, taplo fmt, cargo-audit, cargo-deny, nextest, hakari verification.

**Step 3: Fix any failures**

Address clippy warnings, formatting issues, or test failures.

**Step 4: Update CLAUDE.md**

Update the architecture section in `CLAUDE.md` to reflect:
- `StoreError<A, C, U>` is now generic
- `UpcastError<U>` is now generic  
- `Upcaster` trait has `type Error`
- `StreamLabel` no longer exists
- `Id::to_label()` exists as a default method
- `FjallError` uses `ArrayString` fields

**Step 5: Final commit**

```bash
git add -A
git commit -m "docs: update CLAUDE.md architecture for allocation-free error types"
```
