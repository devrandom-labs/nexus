# QA Bugfixes: Save Atomicity, Schema Version, Conflict Errors

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix 5 failing QA tests caused by 3 distinct bugs: save atomicity (d2×3), upcaster double-application (d4×1), conflict error double-wrapping (d8×1).

**Architecture:** Three layered fixes: (1) Add borrow/commit accessors to `AggregateRoot` so the save path can encode without draining; (2) Add `schema_version` to `PendingEnvelope` and probe the upcaster chain on save to stamp events at the correct version; (3) Replace opaque `RawEventStore::append` error with structured `AppendError<E>` so conflicts surface as `StoreError::Conflict` instead of `Adapter`.

**Tech Stack:** Rust (edition 2024), nexus kernel crate, nexus-store crate, tokio, thiserror, smallvec.

**Verification command (run after every phase):**

```bash
nix develop --command bash -c "cargo test -p nexus-store -- repository_qa_tests 2>&1"
```

**Full test suite (run at end):**

```bash
nix develop --command bash -c "cargo test --all 2>&1"
nix develop --command bash -c "cargo clippy --all-targets -- --deny warnings 2>&1"
```

---

## Phase 1: Save Atomicity (Bug 1 — d2 tests)

### Task 1.1: Add `uncommitted_events()` and `clear_uncommitted()` to AggregateRoot

**Files:**
- Modify: `crates/nexus/src/aggregate.rs:469-486` (after `take_uncommitted_events`)

**Step 1: Add the two new methods**

Insert after `take_uncommitted_events` (after line 486):

```rust
/// Read-only access to uncommitted events.
///
/// Used by the repository save path to encode events without draining.
/// Call [`clear_uncommitted()`](Self::clear_uncommitted) after successful
/// persistence to advance the version and clear the buffer.
#[must_use]
pub fn uncommitted_events(&self) -> &[VersionedEvent<EventOf<A>>] {
    &self.uncommitted_events
}

/// Advance the persisted version and clear the uncommitted buffer.
///
/// Call this after events have been successfully persisted. The version
/// advances to include all previously uncommitted events.
///
/// # Contract
///
/// Only call after a successful write to the event store. Calling
/// without persistence leaves the aggregate's version ahead of
/// the store — subsequent saves will conflict.
pub fn clear_uncommitted(&mut self) {
    self.version = self.current_version();
    self.uncommitted_events.clear();
}
```

**Step 2: Verify kernel builds**

Run: `nix develop --command bash -c "cargo build -p nexus 2>&1"`
Expected: compiles

### Task 1.2: Refactor `save_with_encoder` for atomicity

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs:65-112`

**Step 1: Replace the `save_with_encoder` function body**

Replace lines 65-112 (the entire `save_with_encoder` function) with:

```rust
#[allow(clippy::expect_used, reason = "checked non-empty above")]
async fn save_with_encoder<A, S, U, F>(
    store: &S,
    upcasters: &U,
    encode_fn: F,
    stream_id: &str,
    aggregate: &mut AggregateRoot<A>,
) -> Result<(), StoreError>
where
    A: Aggregate,
    S: RawEventStore,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
    F: Fn(&EventOf<A>) -> Result<Vec<u8>, StoreError>,
{
    let _ = upcasters; // upcasters not used on write path (yet — Phase 2 adds schema version)
    let events = aggregate.uncommitted_events();
    if events.is_empty() {
        return Ok(());
    }

    let expected_version = Version::from_persisted(
        events
            .first()
            .expect("checked non-empty above")
            .version()
            .as_u64()
            - 1,
    );

    let mut envelopes = Vec::with_capacity(events.len());
    for ve in events {
        let payload = encode_fn(ve.event())?;
        envelopes.push(
            pending_envelope(stream_id.into())
                .version(ve.version())
                .event_type(ve.event().name())
                .payload(payload)
                .build_without_metadata(),
        );
    }

    store
        .append(stream_id, expected_version, &envelopes)
        .await
        .map_err(|e| StoreError::Adapter(Box::new(e)))?;

    // Success — now safe to advance the aggregate's version
    aggregate.clear_uncommitted();

    Ok(())
}
```

**Step 2: Run d2 tests**

Run: `nix develop --command bash -c "cargo test -p nexus-store -- d2_ 2>&1"`
Expected: all 3 d2 tests PASS

**Step 3: Run full QA suite to check for regressions**

Run: `nix develop --command bash -c "cargo test -p nexus-store -- repository_qa_tests 2>&1"`
Expected: d2 tests pass, d4 and d8 still fail (not yet fixed), no new regressions

---

## Phase 2: Schema Version on Write (Bug 2 — d4 test)

### Task 2.1: Add `schema_version` to `PendingEnvelope`

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

**Step 1: Add `schema_version` field to `PendingEnvelope` struct (line 23-29)**

Replace:
```rust
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    metadata: M,
}
```

With:
```rust
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    schema_version: u32,
    payload: Vec<u8>,
    metadata: M,
}
```

**Step 2: Add accessor to PendingEnvelope impl (after line 55)**

Add after the `metadata()` accessor:

```rust
/// The schema version of the event.
///
/// Defaults to 1 when not explicitly set via the builder.
/// The `EventStore` save path sets this based on the upcaster chain
/// to prevent upcasters from double-transforming new events.
#[must_use]
pub const fn schema_version(&self) -> u32 {
    self.schema_version
}
```

**Step 3: Add `schema_version` field to `WithPayload` struct (line 86-91)**

Replace:
```rust
pub struct WithPayload {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
}
```

With:
```rust
pub struct WithPayload {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    schema_version: u32,
}
```

**Step 4: Update `WithEventType::payload()` to initialize schema_version (line 125)**

Replace:
```rust
pub fn payload(self, payload: Vec<u8>) -> WithPayload {
    WithPayload {
        stream_id: self.stream_id,
        version: self.version,
        event_type: self.event_type,
        payload,
    }
}
```

With:
```rust
pub fn payload(self, payload: Vec<u8>) -> WithPayload {
    WithPayload {
        stream_id: self.stream_id,
        version: self.version,
        event_type: self.event_type,
        payload,
        schema_version: 1,
    }
}
```

**Step 5: Add optional `schema_version()` setter on `WithPayload` (before `build`)**

Add before the `build` method (before line 146):

```rust
/// Override the schema version (default: 1).
///
/// The `EventStore` save path uses this to stamp new events at the
/// current schema version, preventing upcasters from re-transforming
/// events that are already in the latest format.
#[must_use]
pub const fn schema_version(mut self, schema_version: u32) -> Self {
    self.schema_version = schema_version;
    self
}
```

**Step 6: Update `build()` and `build_without_metadata()` to include schema_version**

Replace `build` (line 146-153):
```rust
pub fn build<M>(self, metadata: M) -> PendingEnvelope<M> {
    PendingEnvelope {
        stream_id: self.stream_id,
        version: self.version,
        event_type: self.event_type,
        schema_version: self.schema_version,
        payload: self.payload,
        metadata,
    }
}
```

(`build_without_metadata` calls `self.build(())` — no change needed for it.)

**Step 7: Verify it compiles**

Run: `nix develop --command bash -c "cargo build -p nexus-store 2>&1"`
Expected: compiles (existing callers get default schema_version=1)

### Task 2.2: Add `can_upcast` to `UpcasterChain` trait

**Files:**
- Modify: `crates/nexus-store/src/upcaster_chain.rs`

**Step 1: Add `can_upcast` to the trait and both impls**

Replace the entire file with:

```rust
use crate::upcaster::EventUpcaster;

/// A link in the upcaster chain. Fully monomorphized, zero-size when
/// upcasters are stateless (unit structs).
pub struct Chain<H, T>(pub H, pub T);

/// Compile-time upcaster chain. Dispatches to the first matching
/// upcaster in the chain. The compiler inlines the entire traversal.
pub trait UpcasterChain: Send + Sync {
    /// Try each upcaster in order. Returns `Some(...)` from the first
    /// match, or `None` if no upcaster handles this event type + version.
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)>;

    /// Check if any upcaster in the chain handles this event type + version.
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;
}

/// Base case: empty chain. Always returns `None`.
impl UpcasterChain for () {
    #[inline]
    fn try_upcast(&self, _: &str, _: u32, _: &[u8]) -> Option<(String, u32, Vec<u8>)> {
        None
    }

    #[inline]
    fn can_upcast(&self, _: &str, _: u32) -> bool {
        false
    }
}

/// Recursive case: try head, fall through to tail.
impl<H: EventUpcaster, T: UpcasterChain> UpcasterChain for Chain<H, T> {
    #[inline]
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)> {
        if self.0.can_upcast(event_type, schema_version) {
            Some(self.0.upcast(event_type, schema_version, payload))
        } else {
            self.1.try_upcast(event_type, schema_version, payload)
        }
    }

    #[inline]
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        self.0.can_upcast(event_type, schema_version)
            || self.1.can_upcast(event_type, schema_version)
    }
}
```

### Task 2.3: Add `current_schema_version` helper and update save path

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`

**Step 1: Add `current_schema_version` function after `run_upcasters`**

Insert after `run_upcasters` (after line 60):

```rust
/// Determine the "current" schema version for an event type by probing
/// the upcaster chain. Walks versions from 1 upward until no upcaster
/// matches. New events should be stored at this version so that
/// upcasters skip them on load.
fn current_schema_version<U: UpcasterChain>(upcasters: &U, event_type: &str) -> u32 {
    let mut version = 1u32;
    while upcasters.can_upcast(event_type, version) {
        version = version.saturating_add(1);
        if version > UPCAST_CHAIN_LIMIT + 1 {
            break;
        }
    }
    version
}
```

**Step 2: Update `save_with_encoder` to stamp schema version**

In the `save_with_encoder` function (from Task 1.2), replace the line:
```rust
let _ = upcasters; // upcasters not used on write path (yet — Phase 2 adds schema version)
```

Remove it entirely (upcasters is now used).

And replace the envelope construction loop:
```rust
    let mut envelopes = Vec::with_capacity(events.len());
    for ve in events {
        let payload = encode_fn(ve.event())?;
        envelopes.push(
            pending_envelope(stream_id.into())
                .version(ve.version())
                .event_type(ve.event().name())
                .payload(payload)
                .build_without_metadata(),
        );
    }
```

With:
```rust
    let mut envelopes = Vec::with_capacity(events.len());
    for ve in events {
        let payload = encode_fn(ve.event())?;
        let sv = current_schema_version(upcasters, ve.event().name());
        envelopes.push(
            pending_envelope(stream_id.into())
                .version(ve.version())
                .event_type(ve.event().name())
                .payload(payload)
                .schema_version(sv)
                .build_without_metadata(),
        );
    }
```

### Task 2.4: Update `InMemoryStore` to store provided schema_version

**Files:**
- Modify: `crates/nexus-store/src/testing.rs:150-156`

**Step 1: Replace hardcoded `schema_version: 1` with envelope value**

Replace (line 150-156):
```rust
        for env in envelopes {
            stream.push(StoredRow {
                version: env.version().as_u64(),
                event_type: env.event_type().to_owned(),
                schema_version: 1,
                payload: env.payload().to_vec(),
            });
        }
```

With:
```rust
        for env in envelopes {
            stream.push(StoredRow {
                version: env.version().as_u64(),
                event_type: env.event_type().to_owned(),
                schema_version: env.schema_version(),
                payload: env.payload().to_vec(),
            });
        }
```

### Task 2.5: Update tests affected by schema version fix

**Files:**
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`

**Step 1: Update `d4_zero_copy_event_store_with_payload_mutating_upcaster`**

This test currently expects upcasters to fire on newly-saved events (the bug). With the fix, new events are stored at the current schema version and upcasters skip them. Rewrite to test upcasters on OLD events by saving directly via the raw store at schema_version=1.

Replace the test (lines 779-799) with:

```rust
#[tokio::test]
async fn d4_zero_copy_event_store_with_payload_mutating_upcaster() {
    // DeltaDoublingUpcaster doubles the i32 payload on read.
    // This exercises the ZeroCopyEventStore + upcaster path where
    // run_upcasters returns a NEW Vec<u8> and BorrowingCodec::decode
    // borrows from that new buffer.
    //
    // We save a "legacy" event at schema_version=1 via the raw store,
    // then load via ZeroCopyEventStore which applies the upcaster.
    let raw_store = InMemoryStore::new();

    // Write a v1 event directly (simulating a legacy event)
    let legacy = pending_envelope("zc-upcast".into())
        .version(Version::from_persisted(1))
        .event_type("Delta")
        .payload(5_i32.to_le_bytes().to_vec())
        .build_without_metadata(); // schema_version defaults to 1
    raw_store
        .append("zc-upcast", Version::INITIAL, &[legacy])
        .await
        .unwrap();

    // Load via ZeroCopyEventStore with upcaster — delta=5 doubled to 10
    let es = ZeroCopyEventStore::new(raw_store, DeltaBorrowingCodec)
        .with_upcaster(DeltaDoublingUpcaster);
    let loaded: AggregateRoot<DeltaAggregate> = es.load("zc-upcast", CounterId(1)).await.unwrap();
    assert_eq!(
        loaded.state().total, 10,
        "upcaster should double the legacy delta from 5 to 10"
    );
}
```

NOTE: This requires `pending_envelope` import which is already at the top via `nexus_store::...` or will be added. Check the test file imports.

**Step 2: Update `d11_schema_version_always_one` test and comment**

Replace lines 1299-1339 with:

```rust
#[tokio::test]
async fn d11_schema_version_always_one() {
    use nexus_store::pending_envelope;
    use nexus_store::stream::EventStream;

    // InMemoryStore preserves the schema_version from PendingEnvelope.
    // The builder defaults to schema_version=1 when not explicitly set.
    let store = InMemoryStore::new();

    let envelopes = [
        pending_envelope("sv".into())
            .version(Version::from_persisted(1))
            .event_type("Incremented")
            .payload(b"inc".to_vec())
            .build_without_metadata(),
        pending_envelope("sv".into())
            .version(Version::from_persisted(2))
            .event_type("Set")
            .payload(b"set:42".to_vec())
            .build_without_metadata(),
    ];
    store
        .append("sv", Version::INITIAL, &envelopes)
        .await
        .unwrap();

    // Read raw stream — default schema_version is 1
    let mut stream = store.read_stream("sv", Version::INITIAL).await.unwrap();
    let env1 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        env1.schema_version(),
        1,
        "default schema_version should be 1"
    );
    let env2 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        env2.schema_version(),
        1,
        "default schema_version should be 1"
    );
    assert!(stream.next().await.is_none());
}
```

**Step 3: Run d4 tests**

Run: `nix develop --command bash -c "cargo test -p nexus-store -- d4_ 2>&1"`
Expected: all d4 tests PASS

---

## Phase 3: Structured Append Errors (Bug 3 — d8 test)

### Task 3.1: Add `AppendError<E>` to error module

**Files:**
- Modify: `crates/nexus-store/src/error.rs`

**Step 1: Add `AppendError` enum after `StoreError`**

Insert after `StoreError` definition (after line 32):

```rust
/// Structured error from [`RawEventStore::append`](crate::RawEventStore::append).
///
/// Separates concurrency conflicts (a normal, expected condition in
/// optimistic concurrency) from adapter-level failures (I/O, connection).
/// This lets the `EventStore` facade map conflicts to
/// [`StoreError::Conflict`] without opaque wrapping.
#[derive(Debug)]
pub enum AppendError<E> {
    /// Optimistic concurrency conflict — expected version doesn't match.
    Conflict {
        stream_id: ErrorId,
        expected: Version,
        actual: Version,
    },
    /// Adapter-level failure (I/O, serialization, connection, etc.).
    Store(E),
}

impl<E: std::fmt::Display> std::fmt::Display for AppendError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict {
                stream_id,
                expected,
                actual,
            } => write!(
                f,
                "Concurrency conflict on '{stream_id}': expected version {expected}, actual {actual}"
            ),
            Self::Store(e) => write!(f, "Store error: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for AppendError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Conflict { .. } => None,
            Self::Store(e) => Some(e),
        }
    }
}
```

**Step 2: Export `AppendError` from `lib.rs`**

In `crates/nexus-store/src/lib.rs`, update line 17:

Replace:
```rust
pub use error::{InvalidSchemaVersion, StoreError, UpcastError};
```

With:
```rust
pub use error::{AppendError, InvalidSchemaVersion, StoreError, UpcastError};
```

### Task 3.2: Update `RawEventStore::append` return type

**Files:**
- Modify: `crates/nexus-store/src/raw.rs`

**Step 1: Add import and update trait signature**

Replace line 1:
```rust
use crate::envelope::PendingEnvelope;
```

With:
```rust
use crate::envelope::PendingEnvelope;
use crate::error::AppendError;
```

Replace the `append` method signature (lines 37-42):
```rust
    fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
```

With:
```rust
    fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), AppendError<Self::Error>>> + Send;
```

### Task 3.3: Update `InMemoryStore::append`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

**Step 1: Add import**

Add to the imports at top of file (after line 4):
```rust
use crate::error::AppendError;
```

**Step 2: Update append signature and conflict returns**

Replace the append implementation (lines 116-160):

```rust
    #[allow(
        clippy::significant_drop_tightening,
        reason = "guard must be held across concurrency check + push"
    )]
    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();

        // Optimistic concurrency check.
        let actual_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if actual_version != expected_version.as_u64() {
            return Err(AppendError::Conflict {
                stream_id: ErrorId::from_display(&stream_id),
                expected: expected_version,
                actual: Version::from_persisted(actual_version),
            });
        }

        // Sequential version validation: each envelope must have version
        // expected_version + 1 + i.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_version = expected_version.as_u64() + 1 + i_u64;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: Version::from_persisted(expected_env_version),
                    actual: env.version(),
                });
            }
        }

        // Store the events.
        for env in envelopes {
            stream.push(StoredRow {
                version: env.version().as_u64(),
                event_type: env.event_type().to_owned(),
                schema_version: env.schema_version(),
                payload: env.payload().to_vec(),
            });
        }

        Ok(())
    }
```

### Task 3.4: Update `save_with_encoder` error handling

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`

**Step 1: Add import**

Add to the imports at the top of the file:
```rust
use crate::error::AppendError;
```

**Step 2: Replace the append + error mapping block in `save_with_encoder`**

Replace:
```rust
    store
        .append(stream_id, expected_version, &envelopes)
        .await
        .map_err(|e| StoreError::Adapter(Box::new(e)))?;

    // Success — now safe to advance the aggregate's version
    aggregate.clear_uncommitted();

    Ok(())
```

With:
```rust
    match store
        .append(stream_id, expected_version, &envelopes)
        .await
    {
        Ok(()) => {
            aggregate.clear_uncommitted();
            Ok(())
        }
        Err(AppendError::Conflict {
            stream_id,
            expected,
            actual,
        }) => Err(StoreError::Conflict {
            stream_id,
            expected,
            actual,
        }),
        Err(AppendError::Store(e)) => Err(StoreError::Adapter(Box::new(e))),
    }
```

### Task 3.5: Update `ProbeStore` in bug_hunt_tests

**Files:**
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs:87-121`

**Step 1: Add import at top of file**

Add `AppendError` to the imports. Find the existing imports (look for `use nexus_store::error::StoreError` or similar) and add:
```rust
use nexus_store::error::AppendError;
```

**Step 2: Update ProbeStore::append return type and conflict returns**

Replace the append method (lines 94-121):

```rust
    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current != expected_version.as_u64() {
            return Err(AppendError::Store(ProbeError::Conflict));
        }
        // Enforce implementor contract: versions must be sequential
        // from expected_version + 1
        for (expected_next, env) in (expected_version.as_u64() + 1..).zip(envelopes.iter()) {
            if env.version().as_u64() != expected_next {
                return Err(AppendError::Store(ProbeError::Conflict));
            }
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        Ok(())
    }
```

NOTE: `ProbeStore` is a test-internal store. Its conflicts are errors in the adapter logic, not true OCC conflicts, so wrapping in `AppendError::Store` is correct. If you need true OCC conflict semantics, use `AppendError::Conflict { .. }` instead.

### Task 3.6: Update `InMemoryRawStore` in benchmarks

**Files:**
- Modify: `crates/nexus-store/benches/store_bench.rs:88-113`

**Step 1: Add import**

Add near the top imports:
```rust
use nexus_store::error::AppendError;
```

**Step 2: Update append signature and return types**

Replace the append method (lines 92-113):

```rust
    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_version != expected_version.as_u64() {
            return Err(AppendError::Store(BenchError::Conflict));
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        drop(guard);
        Ok(())
    }
```

### Task 3.7: Update tests that match on raw store append errors

**Files:**
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs` (d3 concurrent test)

**Step 1: Update d3_concurrent_saves_one_wins_one_conflicts (lines 633-656)**

This test currently documents the BUG (matching `StoreError::Adapter`). After fix, it should expect `StoreError::Conflict`.

Replace lines 633-656:
```rust
    // Exactly one should succeed, one should fail.
    // The failure is StoreError::Adapter(Box<StoreError::Conflict>) because
    // InMemoryStore returns StoreError which EventStore wraps in Adapter.
    let successes = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_ok())
        .count();
    let failures = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_err())
        .count();

    assert_eq!(successes, 1, "exactly one writer should succeed");
    assert_eq!(failures, 1, "exactly one writer should fail with conflict");

    // Verify the failure is a conflict (wrapped in Adapter)
    let err = match (result_a, result_b) {
        (Err(e), _) | (_, Err(e)) => e,
        _ => unreachable!("one result must be Err"),
    };
    assert!(
        matches!(err, StoreError::Adapter(_)),
        "conflict should be wrapped as StoreError::Adapter: {err:?}"
    );
```

With:
```rust
    // Exactly one should succeed, one should fail with a conflict.
    let successes = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_ok())
        .count();
    let failures = [&result_a, &result_b]
        .iter()
        .filter(|r| r.is_err())
        .count();

    assert_eq!(successes, 1, "exactly one writer should succeed");
    assert_eq!(failures, 1, "exactly one writer should fail with conflict");

    // Conflict must surface as StoreError::Conflict, not wrapped in Adapter
    let err = match (result_a, result_b) {
        (Err(e), _) | (_, Err(e)) => e,
        _ => unreachable!("one result must be Err"),
    };
    assert!(
        matches!(err, StoreError::Conflict { .. }),
        "conflict should be StoreError::Conflict: {err:?}"
    );
```

### Task 3.8: Update examples that call `append` (if needed)

**Files to check:**
- `examples/store-and-kernel/src/main.rs` — calls `.append().await.unwrap()` — no change needed
- `examples/inmemory/src/main.rs` — calls `.append().await.unwrap()` — no change needed

(These just `.unwrap()` the result, so `AppendError` works as long as it impls `Debug`, which it does.)

**Note:** If there's a `examples/store-inmemory/` directory, check it too — same pattern.

### Task 3.9: Run d8 test

Run: `nix develop --command bash -c "cargo test -p nexus-store -- d8_ 2>&1"`
Expected: all d8 tests PASS

---

## Phase 4: Final Verification

### Task 4.1: Run full QA suite

Run: `nix develop --command bash -c "cargo test -p nexus-store -- repository_qa_tests 2>&1"`
Expected: 41 passed, 0 failed

### Task 4.2: Run all tests across workspace

Run: `nix develop --command bash -c "cargo test --all 2>&1"`
Expected: all pass

### Task 4.3: Run clippy

Run: `nix develop --command bash -c "cargo clippy --all-targets -- --deny warnings 2>&1"`
Expected: no warnings

### Task 4.4: Run fmt check

Run: `nix develop --command bash -c "cargo fmt --all --check 2>&1"`
Expected: no changes needed (run `cargo fmt --all` if not)

### Task 4.5: Commit

```bash
git add crates/nexus/src/aggregate.rs \
       crates/nexus-store/src/envelope.rs \
       crates/nexus-store/src/error.rs \
       crates/nexus-store/src/event_store.rs \
       crates/nexus-store/src/lib.rs \
       crates/nexus-store/src/raw.rs \
       crates/nexus-store/src/testing.rs \
       crates/nexus-store/src/upcaster_chain.rs \
       crates/nexus-store/tests/repository_qa_tests.rs \
       crates/nexus-store/tests/bug_hunt_tests.rs \
       crates/nexus-store/benches/store_bench.rs
git commit -m "fix(store)!: save atomicity, schema versioning, and conflict errors

Three correctness fixes for the event store save path:

1. Save atomicity: save_with_encoder now borrows uncommitted events
   (via new uncommitted_events() accessor) and only advances the
   aggregate version after successful persistence. Failed saves no
   longer drain events or advance versions.

2. Schema version on write: PendingEnvelope now carries schema_version.
   The save path probes the UpcasterChain to determine the current
   schema version per event type, preventing upcasters from
   double-transforming newly-saved events on reload.

3. Structured append errors: RawEventStore::append now returns
   AppendError<E> with explicit Conflict variant. Concurrency
   conflicts surface as StoreError::Conflict instead of being
   opaquely wrapped in StoreError::Adapter.

BREAKING CHANGE: RawEventStore::append return type changed from
Result<(), Self::Error> to Result<(), AppendError<Self::Error>>.
Implementors must wrap errors in AppendError::Conflict or
AppendError::Store."
```
