# Remove StreamId Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Delete `StreamId` from the kernel. `RawEventStore` takes `&impl Id`. `Repository` takes only the aggregate ID. `PendingEnvelope` and `PersistedEnvelope` drop `stream_id`.

**Architecture:** Bottom-up — kernel first (delete type), then envelope (remove field), then traits (new signatures), then facades + adapters, then tests.

**Tech Stack:** Rust, nexus kernel, nexus-store, nexus-fjall

---

### Task 1: Delete StreamId from kernel

**Files:**
- Delete: `crates/nexus/src/stream_id.rs`
- Modify: `crates/nexus/src/lib.rs:7,19`

**Step 1: Delete the stream_id module**

Delete `crates/nexus/src/stream_id.rs` entirely.

**Step 2: Remove from lib.rs**

In `crates/nexus/src/lib.rs`, remove:
```rust
mod stream_id;
```
and:
```rust
pub use stream_id::{InvalidStreamId, MAX_STREAM_ID_LEN, StreamId};
```

**Step 3: Verify kernel compiles**

Run: `cargo check -p nexus`
Expected: PASS (nothing in the kernel uses StreamId internally)

**Step 4: Commit**

```
feat(nexus)!: delete StreamId from kernel
```

---

### Task 2: Remove stream_id from PendingEnvelope

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

**Step 1: Remove stream_id from PendingEnvelope struct**

Remove `stream_id: StreamId` field from `PendingEnvelope<M>` (line 26).
Remove `stream_id()` accessor (lines 36-39).
Remove `use nexus::StreamId` import (line 2).

**Step 2: Remove typestate builder steps involving stream_id**

The builder currently starts with `WithStreamId`. Restructure:
- Delete `WithStreamId` struct (lines 81-83)
- Delete `WithStreamId::new` and `WithStreamId::version` (lines 107-122)
- `pending_envelope()` no longer takes a `StreamId`. The builder starts at version directly.

New builder entry point:
```rust
pub struct PendingEnvelopeBuilder;

impl PendingEnvelopeBuilder {
    pub fn version(version: Version) -> WithVersion {
        WithVersion { version }
    }
}
```

Or simpler — just make `WithVersion` constructable directly:
```rust
pub const fn pending_envelope(version: Version) -> WithVersion {
    WithVersion { version }
}
```

Update `WithVersion` to not carry `stream_id`. Cascade removal through `WithEventType` and `WithPayload` — remove `stream_id` field from each.

**Step 3: Update PendingEnvelope construction in build()**

`WithPayload::build()` no longer sets `stream_id`:
```rust
pub fn build<M>(self, metadata: M) -> PendingEnvelope<M> {
    PendingEnvelope {
        version: self.version,
        event_type: self.event_type,
        schema_version: self.schema_version,
        payload: self.payload,
        metadata,
    }
}
```

**Step 4: Verify nexus-store compiles (expect errors in event_store.rs, testing.rs)**

Run: `cargo check -p nexus-store 2>&1 | head -40`
Expected: Errors in files that still pass StreamId — that's Task 4.

---

### Task 3: Remove stream_id from PersistedEnvelope

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

**Step 1: Remove stream_id field and accessor**

Remove `stream_id: &'a str` from `PersistedEnvelope<'a, M>` (line 220).
Remove `stream_id()` accessor (lines 298-300).

**Step 2: Update constructors**

Remove `stream_id` parameter from `new_unchecked()` and `try_new()`:

```rust
pub const fn new_unchecked(
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
) -> Self { ... }

pub fn try_new(
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
) -> Result<Self, InvalidSchemaVersion> { ... }
```

---

### Task 4: Update RawEventStore trait

**Files:**
- Modify: `crates/nexus-store/src/raw.rs`

**Step 1: Change trait signatures**

```rust
use nexus::Id;
use nexus::Version;

pub trait RawEventStore<M = ()>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a where Self: 'a;

    fn append(
        &self,
        id: &(impl Id + Send + Sync),
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), AppendError<Self::Error>>> + Send;

    fn read_stream(
        &self,
        id: &(impl Id + Send + Sync),
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream<'_>, Self::Error>> + Send;
}
```

Note: `expected_version` changes from `Version` to `Option<Version>`. `None` means new stream (no events yet). `Some(v)` means the stream was last seen at version `v`.

---

### Task 5: Update Repository trait

**Files:**
- Modify: `crates/nexus-store/src/repository.rs`

**Step 1: Simplify signatures**

```rust
use nexus::{Aggregate, AggregateRoot};
use std::future::Future;

pub trait Repository<A: Aggregate>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load(
        &self,
        id: A::Id,
    ) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

Remove `use nexus::StreamId`.

---

### Task 6: Update nexus-store lib.rs re-exports

**Files:**
- Modify: `crates/nexus-store/src/lib.rs:19`

**Step 1: Remove StreamId re-export**

Remove:
```rust
pub use nexus::StreamId;
```

---

### Task 7: Update EventStore and ZeroCopyEventStore facades

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`

**Step 1: Update save_with_encoder**

Remove `stream_id` parameter. Get the ID from the aggregate:

```rust
async fn save_with_encoder<A, S, U, F>(
    store: &S,
    upcasters: &U,
    encode_fn: F,
    aggregate: &mut AggregateRoot<A>,
) -> Result<(), StoreError>
where
    A: Aggregate,
    S: RawEventStore,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
    F: Fn(&EventOf<A>) -> Result<Vec<u8>, StoreError>,
{
    // ...
    // expected_version derived from aggregate.version() (Option<Version>)
    // id from aggregate.id()
    // pending_envelope() no longer takes stream_id
    // store.append(aggregate.id(), expected_version, &envelopes)
}
```

**Step 2: Update Repository impl for EventStore**

```rust
async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
    let mut stream = self.store
        .read_stream(&id, Version::INITIAL)
        .await
        .map_err(|e| StoreError::Adapter(Box::new(e)))?;
    let mut root = AggregateRoot::<A>::new(id);
    // ... replay loop unchanged ...
    Ok(root)
}

async fn save(&self, aggregate: &mut AggregateRoot<A>) -> Result<(), StoreError> {
    save_with_encoder(&self.store, &self.upcasters, |event| { ... }, aggregate).await
}
```

**Step 3: Same changes for ZeroCopyEventStore**

Mirror the EventStore changes.

**Step 4: Remove StreamId import**

Remove `use nexus::StreamId` from the top of the file.

**Step 5: Verify nexus-store compiles**

Run: `cargo check -p nexus-store`

---

### Task 8: Update InMemoryStore (testing.rs)

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

**Step 1: Update append and read_stream signatures**

Use `id.to_string()` as the HashMap key instead of `stream_id.as_str()`.

Remove `stream_id` from `ReadRow` struct.
Remove `use nexus::StreamId`.

Update `PersistedEnvelope` construction to drop the `stream_id` argument.

**Step 2: Verify testing feature compiles**

Run: `cargo check -p nexus-store --features testing`

---

### Task 9: Update FjallStore

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`

**Step 1: Update append signature**

Replace `stream_id: &StreamId` with `id: &(impl Id + Send + Sync)`.
Use `id.to_string()` where the code currently uses `stream_id.as_str()`.
Update `ErrorId::from_display(id)` calls (already correct pattern).

**Step 2: Handle expected_version: Option<Version>**

Replace the current `expected_version.as_u64() != 0` check for new streams with a match on `Option<Version>`:
- `None` → new stream, allocate numeric ID
- `Some(v)` → existing stream, check version matches

**Step 3: Update read_stream signature**

Replace `stream_id: &StreamId` with `id: &(impl Id + Send + Sync)`.
Use `id.to_string()` for the streams partition lookup.

**Step 4: Remove StreamId import**

Remove `use nexus::StreamId` from the import block.

**Step 5: Update inline tests**

Update `sid()` helper and `make_envelope()` to work without StreamId.

---

### Task 10: Update FjallStream

**Files:**
- Modify: `crates/nexus-fjall/src/stream.rs`

**Step 1: Keep stream_id: String field for error diagnostics**

`FjallStream` uses `stream_id` for error messages in `CorruptValue`. This is fine — it's the stringified ID, not a `StreamId` type. The field name could be renamed to `id_display` or kept as-is. The value comes from `id.to_string()` in `read_stream`.

**Step 2: Remove stream_id from PersistedEnvelope construction**

Update the `PersistedEnvelope::try_new` call to drop the `stream_id` argument.

**Step 3: Update inline tests**

Remove `stream_id` assertions from envelope checks.

---

### Task 11: Update FjallError

**Files:**
- Modify: `crates/nexus-fjall/src/error.rs`

**Step 1: Rename `StreamIdExhausted` to `IdSpaceExhausted`**

This error is about numeric ID allocation exhaustion, not "stream IDs". Rename for clarity.

---

### Task 12: Update all nexus-store tests

**Files:**
- Modify: all files in `crates/nexus-store/tests/`

**Step 1: Bulk update pattern**

Across all test files:
- Remove `use nexus::StreamId` / `use nexus_store::StreamId`
- Remove `StreamId::new(...)` / `StreamId::from_persisted(...)` calls
- Replace with a simple ID type (e.g., the existing test aggregate ID types, or a simple `&str` / `String` that implements `Id`)
- Remove `stream_id` from `pending_envelope()` calls — now starts with `.version()`
- Remove `.stream_id()` assertions from `PersistedEnvelope` checks

For tests that need a simple ID, use a test helper:
```rust
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl std::fmt::Display for TestId { ... }
impl nexus::Id for TestId {}
```

**Step 2: Run all nexus-store tests**

Run: `cargo test -p nexus-store`

---

### Task 13: Update all nexus-fjall tests

**Files:**
- Modify: all files in `crates/nexus-fjall/tests/`

**Step 1: Same bulk update pattern as Task 12**

Replace `StreamId` usage with direct ID types. Update `pending_envelope()` calls. Remove `stream_id()` assertions.

**Step 2: Run all nexus-fjall tests**

Run: `cargo test -p nexus-fjall`

---

### Task 14: Update benchmarks

**Files:**
- Modify: `crates/nexus-store/benches/store_bench.rs`
- Modify: `crates/nexus-fjall/benches/fjall_benchmarks.rs`

**Step 1: Same pattern — replace StreamId with direct ID types**

**Step 2: Verify benchmarks compile**

Run: `cargo check --benches -p nexus-store -p nexus-fjall`

---

### Task 15: Update examples

**Files:**
- Check and update any examples in `examples/` that use StreamId

**Step 1: Search for StreamId in examples**

Run: `grep -r StreamId examples/`

**Step 2: Update if found**

---

### Task 16: Update compile-fail tests

**Files:**
- Modify: `crates/nexus-store/tests/compile_fail/builder_incomplete.rs`
- Modify: `crates/nexus-store/tests/compile_fail/builder_skip_version.rs`

**Step 1: Update builder usage**

The builder no longer starts with a StreamId. Update the compile-fail tests to reflect the new builder API.

**Step 2: Bless updated compiler output**

Run: `TRYBUILD=overwrite cargo test -p nexus-store -- compile_fail`

---

### Task 17: Full verification

**Step 1: Run all tests**

Run: `cargo test --all`

**Step 2: Clippy**

Run: `cargo clippy --all-targets -- --deny warnings`

**Step 3: Format**

Run: `cargo fmt --all --check`

**Step 4: Commit**

```
refactor(store)!: remove StreamId, RawEventStore takes &impl Id
```
