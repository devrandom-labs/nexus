# Event Subscription Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add per-stream catch-up + live subscriptions to Nexus, with checkpoint persistence for at-least-once delivery.

**Architecture:** Two new traits (`Subscription`, `CheckpointStore`) in `nexus-store` define the contract. Adapters (`InMemoryStore`, `FjallStore`) implement them using `tokio::sync::Notify` for zero-allocation notification. Subscription streams reuse the existing `EventStream` GAT lending iterator — they just never return `None`, blocking instead when caught up.

**Tech Stack:** Rust, `tokio::sync::Notify`, fjall transactions, existing `EventStream` GAT pattern.

**Design doc:** `docs/plans/2026-04-13-subscription-design.md`

**Mandatory rules:** See `CLAUDE.md` — especially sections on atomicity (Rule 1), error handling (Rule 3), API design (Rule 4), functional-first style (Rule 6), and the 4 test categories (Rule 7).

---

### Task 1: Subscription and CheckpointStore Traits in nexus-store

**Files:**
- Create: `crates/nexus-store/src/store/subscription.rs`
- Create: `crates/nexus-store/src/store/checkpoint.rs`
- Modify: `crates/nexus-store/src/store/mod.rs` (add module declarations and re-exports)
- Modify: `crates/nexus-store/src/lib.rs` (add public re-exports)

**Step 1: Create the `Subscription` trait**

Create `crates/nexus-store/src/store/subscription.rs`:

```rust
use std::future::Future;

use super::stream::EventStream;
use nexus::{Id, Version};

/// A subscription to events in a single stream.
///
/// Returns an [`EventStream`] that never exhausts — it waits for new
/// events when caught up, rather than returning `None`.
///
/// # Contract
///
/// - `from: None` → start from the beginning of the stream (version 1)
/// - `from: Some(v)` → start from the event *after* version `v`
/// - The returned stream **never returns `None`**. When all existing
///   events have been yielded, it blocks until new events are appended.
/// - Events are yielded with monotonically increasing versions, same
///   as [`EventStream`].
///
/// # Difference from `RawEventStore::read_stream`
///
/// `read_stream` returns a fused stream that terminates when caught up.
/// `subscribe` returns a stream that *waits* instead of terminating.
pub trait Subscription<M: 'static> {
    /// The subscription stream type — an [`EventStream`] that never exhausts.
    type Stream<'a>: EventStream<M> + 'a
    where
        Self: 'a;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Subscribe to events in a single stream.
    fn subscribe<'a>(
        &'a self,
        id: &'a (impl Id + Sync),
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a;
}
```

**Step 2: Create the `CheckpointStore` trait**

Create `crates/nexus-store/src/store/checkpoint.rs`:

```rust
use std::future::Future;

use nexus::{Id, Version};

/// Persists subscription positions for resume-after-restart.
///
/// A checkpoint records how far a subscription has processed events
/// in a stream. On restart, the subscriber loads its checkpoint and
/// passes it as `from` to [`Subscription::subscribe`].
///
/// # Contract
///
/// - `load` returns `None` for unknown subscription IDs (not an error).
/// - `save` overwrites the previous checkpoint for the given ID.
/// - Implementations must be durable — a saved checkpoint must survive
///   process restart.
pub trait CheckpointStore {
    /// The error type for checkpoint operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Load the last saved checkpoint for a subscription.
    ///
    /// Returns `None` if no checkpoint has been saved for this ID.
    fn load(
        &self,
        subscription_id: &(impl Id + Sync),
    ) -> impl Future<Output = Result<Option<Version>, Self::Error>> + Send + '_;

    /// Save (overwrite) the checkpoint for a subscription.
    fn save(
        &self,
        subscription_id: &(impl Id + Sync),
        version: Version,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + '_;
}
```

**Step 3: Wire up module declarations and re-exports**

In `crates/nexus-store/src/store/mod.rs`, add module declarations:

```rust
mod checkpoint;
mod subscription;
```

And add re-exports:

```rust
pub use checkpoint::CheckpointStore;
pub use subscription::Subscription;
```

In `crates/nexus-store/src/lib.rs`, add to the existing `pub use store::{...}` block:

```rust
pub use store::{CheckpointStore, Subscription};
```

**Step 4: Verify it compiles**

Run: `cargo build -p nexus-store`

Expected: Compiles with no errors. The traits have no implementors yet — that's fine.

**Step 5: Commit**

```
feat(store): add Subscription and CheckpointStore traits (#125)
```

---

### Task 2: InMemoryStore Subscription Support

**Files:**
- Modify: `crates/nexus-store/src/testing.rs` (add `Notify`, implement `Subscription` and `CheckpointStore`)

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/subscription_tests.rs`:

```rust
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::panic, reason = "test code")]

use nexus::Version;
use nexus_store::store::{EventStream, RawEventStore, Subscription, CheckpointStore};
use nexus_store::testing::InMemoryStore;
use nexus_store::envelope::pending_envelope;
use std::time::Duration;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl nexus::Id for TestId {}

fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

fn make_envelope(
    version: u64,
    event_type: &'static str,
    payload: &[u8],
) -> nexus_store::PendingEnvelope<()> {
    pending_envelope(Version::new(version).expect("test version must be > 0"))
        .event_type(event_type)
        .payload(payload.to_vec())
        .build_without_metadata()
}

// ═══════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════

/// Subscribe catches up on existing events, then receives live events.
#[tokio::test]
async fn subscribe_catchup_then_live() {
    let store = InMemoryStore::new();

    // Pre-populate: 2 events
    store
        .append(&tid("s1"), None, &[make_envelope(1, "A", b"p1"), make_envelope(2, "B", b"p2")])
        .await
        .unwrap();

    let mut stream = store.subscribe(&tid("s1"), None).await.unwrap();

    // Catch-up: should get events 1 and 2
    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version(), Version::new(1).unwrap());
        assert_eq!(e1.event_type(), "A");
    }
    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version(), Version::new(2).unwrap());
        assert_eq!(e2.event_type(), "B");
    }

    // Append a new event while subscribed
    store
        .append(&tid("s1"), Version::new(2), &[make_envelope(3, "C", b"p3")])
        .await
        .unwrap();

    // Live: should receive event 3
    let e3_result = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
    let e3 = e3_result.unwrap().unwrap().unwrap();
    assert_eq!(e3.version(), Version::new(3).unwrap());
    assert_eq!(e3.event_type(), "C");
}

/// Subscribe with a checkpoint skips earlier events.
#[tokio::test]
async fn subscribe_from_checkpoint() {
    let store = InMemoryStore::new();

    store
        .append(
            &tid("s1"),
            None,
            &[make_envelope(1, "A", b""), make_envelope(2, "B", b""), make_envelope(3, "C", b"")],
        )
        .await
        .unwrap();

    // Subscribe from after version 2
    let mut stream = store.subscribe(&tid("s1"), Version::new(2)).await.unwrap();

    let e = stream.next().await.unwrap().unwrap();
    assert_eq!(e.version(), Version::new(3).unwrap());
    assert_eq!(e.event_type(), "C");
}

// ═══════════════════════════════════════════════════════════════════
// 2. Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════

/// Drop subscriber, re-subscribe from checkpoint, get continuity.
#[tokio::test]
async fn drop_and_resubscribe_from_checkpoint() {
    let store = InMemoryStore::new();

    store
        .append(&tid("s1"), None, &[make_envelope(1, "A", b"")])
        .await
        .unwrap();

    // First subscription: read event 1
    {
        let mut stream = store.subscribe(&tid("s1"), None).await.unwrap();
        let e = stream.next().await.unwrap().unwrap();
        assert_eq!(e.version().as_u64(), 1);
        // Subscriber dropped here
    }

    // Append more while no subscriber
    store
        .append(&tid("s1"), Version::new(1), &[make_envelope(2, "B", b"")])
        .await
        .unwrap();

    // Re-subscribe from checkpoint (version 1 = already processed)
    let mut stream = store.subscribe(&tid("s1"), Version::new(1)).await.unwrap();
    let e = stream.next().await.unwrap().unwrap();
    assert_eq!(e.version().as_u64(), 2);
    assert_eq!(e.event_type(), "B");
}

/// Append while no subscriber exists, subscribe later catches up.
#[tokio::test]
async fn catchup_events_appended_before_subscribe() {
    let store = InMemoryStore::new();

    store
        .append(
            &tid("s1"),
            None,
            &[make_envelope(1, "A", b""), make_envelope(2, "B", b"")],
        )
        .await
        .unwrap();

    let mut stream = store.subscribe(&tid("s1"), None).await.unwrap();
    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version().as_u64(), 1);
    }
    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version().as_u64(), 2);
    }
}

// ═══════════════════════════════════════════════════════════════════
// 3. Defensive Boundary Tests
// ═══════════════════════════════════════════════════════════════════

/// Subscribe to nonexistent stream: should block, not error.
/// When an event is later appended, it should be received.
#[tokio::test]
async fn subscribe_to_nonexistent_stream_waits() {
    let store = InMemoryStore::new();

    let mut stream = store.subscribe(&tid("ghost"), None).await.unwrap();

    // Append to the stream after subscribing
    store
        .append(&tid("ghost"), None, &[make_envelope(1, "Appeared", b"")])
        .await
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
    let e = result.unwrap().unwrap().unwrap();
    assert_eq!(e.version().as_u64(), 1);
    assert_eq!(e.event_type(), "Appeared");
}

/// Checkpoint load for unknown ID returns None.
#[tokio::test]
async fn checkpoint_load_unknown_returns_none() {
    let store = InMemoryStore::new();
    let result = store.load(&tid("nonexistent")).await.unwrap();
    assert_eq!(result, None);
}

/// Checkpoint save and load round-trip.
#[tokio::test]
async fn checkpoint_save_load_roundtrip() {
    let store = InMemoryStore::new();
    let v = Version::new(42).unwrap();
    store.save(&tid("sub-1"), v).await.unwrap();
    let loaded = store.load(&tid("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(v));
}

/// Checkpoint save overwrites previous value.
#[tokio::test]
async fn checkpoint_save_overwrites() {
    let store = InMemoryStore::new();
    store
        .save(&tid("sub-1"), Version::new(1).unwrap())
        .await
        .unwrap();
    store
        .save(&tid("sub-1"), Version::new(5).unwrap())
        .await
        .unwrap();
    let loaded = store.load(&tid("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(Version::new(5).unwrap()));
}

// ═══════════════════════════════════════════════════════════════════
// 4. Linearizability/Isolation Tests
// ═══════════════════════════════════════════════════════════════════

/// Concurrent appender + subscriber: subscriber sees all events.
#[tokio::test]
async fn concurrent_append_and_subscribe() {
    let store = InMemoryStore::new();

    let mut stream = store.subscribe(&tid("s1"), None).await.unwrap();

    let append_count = 50u64;

    // Spawn appender
    let store_ref = &store;
    let appender = tokio::spawn({
        // We need the store to be 'static for spawn, so we'll restructure
        // to use Arc. For now, use a scoped approach.
        async {}
    });
    drop(appender);

    // Simpler approach: interleave appends and reads
    for i in 1..=append_count {
        let expected_ver = Version::new(i).unwrap();
        store
            .append(
                &tid("s1"),
                if i == 1 { None } else { Version::new(i - 1) },
                &[make_envelope(i, "E", b"")],
            )
            .await
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
        let e = result.unwrap().unwrap().unwrap();
        assert_eq!(e.version(), expected_ver);
    }
}

/// Multiple subscribers to the same stream see the same events.
#[tokio::test]
async fn multiple_subscribers_same_stream() {
    let store = InMemoryStore::new();

    store
        .append(&tid("s1"), None, &[make_envelope(1, "A", b"")])
        .await
        .unwrap();

    let mut sub1 = store.subscribe(&tid("s1"), None).await.unwrap();
    let mut sub2 = store.subscribe(&tid("s1"), None).await.unwrap();

    {
        let e1 = sub1.next().await.unwrap().unwrap();
        assert_eq!(e1.version().as_u64(), 1);
        assert_eq!(e1.event_type(), "A");
    }
    {
        let e2 = sub2.next().await.unwrap().unwrap();
        assert_eq!(e2.version().as_u64(), 1);
        assert_eq!(e2.event_type(), "A");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-store -- subscription_tests`

Expected: Compilation failure — `InMemoryStore` doesn't implement `Subscription` or `CheckpointStore` yet.

**Step 3: Implement InMemoryStore subscription**

Modify `crates/nexus-store/src/testing.rs`:

1. Add `tokio::sync::Notify` field to `InMemoryStore`
2. Add `checkpoints: Mutex<HashMap<String, Version>>` field
3. Signal `notify.notify_waiters()` at end of `append()`
4. Implement `Subscription` for `InMemoryStore`:
   - `subscribe()` creates an `InMemorySubscriptionStream` holding a reference to the store, the stream id (as `String`), the starting position, and a reference to the `Notify`
   - `InMemorySubscriptionStream` implements `EventStream`:
     - Reads events from the store using the internal `Mutex<HashMap>` (similar to `read_stream`)
     - When no more events, awaits `notify.notified()`, then re-reads from current position
     - Never returns `None`
5. Implement `CheckpointStore` for `InMemoryStore`:
   - `load()` reads from `checkpoints` hashmap
   - `save()` inserts/overwrites in `checkpoints` hashmap

Key implementation notes:
- The subscription stream must hold `&'a InMemoryStore` (borrows the store)
- Track `current_version: Option<Version>` to know where to read from next
- After each `notified()`, re-read from `current_version + 1` (or `Version::INITIAL` if `None`)
- The `EventStream::next()` method cannot return a `PersistedEnvelope` that borrows from both the stream AND the store simultaneously (GAT lending). Solution: eagerly load a batch of events into a local buffer (like `InMemoryStream` does), yield from that buffer, then when exhausted wait on notify and reload.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus-store -- subscription_tests`

Expected: All tests pass.

**Step 5: Run clippy**

Run: `cargo clippy -p nexus-store --all-targets -- --deny warnings`

Expected: No warnings.

**Step 6: Commit**

```
feat(store): InMemoryStore implements Subscription and CheckpointStore (#125)
```

---

### Task 3: FjallStore Notification (Notify on Append)

**Files:**
- Modify: `crates/nexus-fjall/Cargo.toml` (add tokio dependency)
- Modify: `crates/nexus-fjall/src/store.rs` (add `Notify` field, signal after commit)
- Modify: `crates/nexus-fjall/src/builder.rs` (initialize `Notify`)

**Step 1: Add tokio dependency to nexus-fjall**

In `crates/nexus-fjall/Cargo.toml`, add to `[dependencies]`:

```toml
tokio = { workspace = true, features = ["sync"] }
```

**Step 2: Add `Notify` to `FjallStore`**

In `crates/nexus-fjall/src/store.rs`, add field to `FjallStore`:

```rust
pub(crate) notify: tokio::sync::Notify,
```

In `crates/nexus-fjall/src/builder.rs`, add to the `FjallStore` construction in `open()`:

```rust
notify: tokio::sync::Notify::new(),
```

**Step 3: Signal after successful append commit**

In `crates/nexus-fjall/src/store.rs`, after `tx.commit()` succeeds in `append()` (line 178), add:

```rust
self.notify.notify_waiters();
```

Note: Only signal after commit succeeds and only when events were actually written (not on empty batches that return early).

**Step 4: Verify existing tests still pass**

Run: `cargo test -p nexus-fjall`

Expected: All existing tests pass — adding `Notify` doesn't change behavior.

**Step 5: Run hakari to update workspace-hack**

Run: `cargo hakari generate && cargo hakari manage-deps`

**Step 6: Commit**

```
feat(fjall): add tokio::sync::Notify to FjallStore for subscription signaling (#125)
```

---

### Task 4: FjallStore Subscription Implementation

**Files:**
- Create: `crates/nexus-fjall/src/subscription_stream.rs`
- Modify: `crates/nexus-fjall/src/store.rs` (implement `Subscription` trait)
- Modify: `crates/nexus-fjall/src/lib.rs` (add module + re-export)

**Step 1: Write failing tests**

Create `crates/nexus-fjall/tests/subscription_tests.rs`:

Tests should mirror the InMemoryStore tests from Task 2 but against `FjallStore`. Same 4 categories:

1. **Sequence/Protocol:**
   - `subscribe_catchup_then_live` — pre-populate, subscribe, read catch-up events, append new, read live
   - `subscribe_from_checkpoint` — subscribe from a version, verify only later events arrive

2. **Lifecycle:**
   - `drop_and_resubscribe` — subscribe, drop, append more, re-subscribe from checkpoint
   - `write_close_reopen_subscribe` — append events, drop store, reopen from same path, subscribe, verify all events present

3. **Defensive Boundary:**
   - `subscribe_to_nonexistent_stream_waits` — subscribe to empty stream, append later, verify arrival
   - `subscribe_from_beyond_head` — subscribe with `from` version past current head, verify it waits for future events

4. **Linearizability/Isolation:**
   - `concurrent_append_and_subscribe` — interleaved appends and reads
   - `multiple_subscribers_same_stream` — two subscribers see identical events

Each test uses `tempfile::tempdir()` + `FjallStore::builder(dir.path().join("db")).open()` setup, same pattern as existing fjall tests.

Use `tokio::time::timeout(Duration::from_secs(2), stream.next())` to avoid hanging on bugs.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus-fjall -- subscription_tests`

Expected: Compilation failure — `FjallStore` doesn't implement `Subscription`.

**Step 3: Create `FjallSubscriptionStream`**

Create `crates/nexus-fjall/src/subscription_stream.rs`:

```rust
use crate::error::FjallError;
use crate::store::FjallStore;
use crate::stream::FjallStream;
use nexus::Version;
use nexus_store::PersistedEnvelope;
use nexus_store::store::EventStream;
use tokio::sync::Notify;

/// Subscription stream over fjall events.
///
/// Catches up from existing events, then waits on [`Notify`] for live
/// events. Never returns `None` from `next()`.
pub struct FjallSubscriptionStream<'a> {
    store: &'a FjallStore,
    notify: &'a Notify,
    stream_id: String,
    /// The inner batch of eagerly-loaded events from the current read.
    inner: FjallStream,
    /// The last version yielded (tracks position for re-reads).
    last_version: Option<Version>,
}
```

The `EventStream` impl:
1. Try `inner.next()` — if `Some(Ok(env))`, update `last_version` and yield it
2. If `Some(Err(e))`, yield the error
3. If `None` (inner exhausted), await `notify.notified()`
4. After wake, call `store.read_stream(id, last_version.next() or INITIAL)` to get a fresh `FjallStream`
5. Replace `self.inner` with the new stream
6. Go back to step 1

The `FjallSubscriptionStream` constructor must also accept the initial `from` parameter and convert it to the `read_stream` starting version:
- `from: None` → `read_stream(id, Version::INITIAL)` (start from version 1)
- `from: Some(v)` → `read_stream(id, v.next()?)` (start after v; if v is `u64::MAX`, this is a version overflow error)

**Step 4: Implement `Subscription` for `FjallStore`**

In `crates/nexus-fjall/src/store.rs`:

```rust
impl Subscription<()> for FjallStore {
    type Stream<'a> = FjallSubscriptionStream<'a>;
    type Error = FjallError;

    async fn subscribe<'a>(
        &'a self,
        id: &'a (impl Id + Sync),
        from: Option<Version>,
    ) -> Result<Self::Stream<'a>, Self::Error> {
        // Determine starting version for the first read_stream call
        let start = match from {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(/* version overflow error */)?,
        };
        let inner = self.read_stream_by_string(&self.to_string(), start).await?;
        Ok(FjallSubscriptionStream {
            store: self,
            notify: &self.notify,
            stream_id: id.to_string(),
            inner,
            last_version: from,
        })
    }
}
```

Note: `read_stream` takes `&impl Id`, but internally we need to call it with a `String`. You may need to add a `pub(crate)` helper that takes a `&str` directly, or use the existing `id_string` pattern.

**Step 5: Wire up module and re-exports**

In `crates/nexus-fjall/src/lib.rs`:

```rust
pub mod subscription_stream;
pub use subscription_stream::FjallSubscriptionStream;
```

**Step 6: Run tests**

Run: `cargo test -p nexus-fjall -- subscription_tests`

Expected: All pass.

**Step 7: Run clippy**

Run: `cargo clippy -p nexus-fjall --all-targets -- --deny warnings`

Expected: No warnings.

**Step 8: Commit**

```
feat(fjall): implement Subscription for FjallStore (#125)
```

---

### Task 5: FjallStore CheckpointStore Implementation

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` (implement `CheckpointStore`)
- Modify: `crates/nexus-fjall/src/builder.rs` (add `checkpoints` partition)
- Modify: `crates/nexus-fjall/src/encoding.rs` (add checkpoint encoding helpers if needed)

**Step 1: Write failing tests**

Add to `crates/nexus-fjall/tests/subscription_tests.rs`:

```rust
// ── CheckpointStore tests ──

/// Checkpoint load for unknown ID returns None.
#[tokio::test]
async fn checkpoint_load_unknown_returns_none() {
    let (store, _dir) = temp_store();
    let result = store.load(&tid("nonexistent")).await.unwrap();
    assert_eq!(result, None);
}

/// Save and load round-trip.
#[tokio::test]
async fn checkpoint_save_load_roundtrip() {
    let (store, _dir) = temp_store();
    store.save(&tid("sub-1"), Version::new(42).unwrap()).await.unwrap();
    let loaded = store.load(&tid("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(Version::new(42).unwrap()));
}

/// Save overwrites previous value.
#[tokio::test]
async fn checkpoint_save_overwrites() {
    let (store, _dir) = temp_store();
    store.save(&tid("sub-1"), Version::new(1).unwrap()).await.unwrap();
    store.save(&tid("sub-1"), Version::new(5).unwrap()).await.unwrap();
    let loaded = store.load(&tid("sub-1")).await.unwrap();
    assert_eq!(loaded, Some(Version::new(5).unwrap()));
}

/// Checkpoint persists across close/reopen (lifecycle).
#[tokio::test]
async fn checkpoint_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        store.save(&tid("sub-1"), Version::new(10).unwrap()).await.unwrap();
        drop(store);
    }

    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let loaded = store.load(&tid("sub-1")).await.unwrap();
        assert_eq!(loaded, Some(Version::new(10).unwrap()));
    }
}
```

**Step 2: Run tests to verify failure**

Run: `cargo test -p nexus-fjall -- checkpoint`

Expected: Compilation failure.

**Step 3: Add checkpoints partition**

In `crates/nexus-fjall/src/store.rs`, add field:

```rust
pub(crate) checkpoints: fjall::TxPartitionHandle,
```

In `crates/nexus-fjall/src/builder.rs` `open()`, add partition creation (similar to snapshots but always present, not feature-gated):

```rust
let checkpoints = db.open_partition(
    "checkpoints",
    PartitionCreateOptions::default()
        .block_size(4_096)
        .bloom_filter_bits(Some(15)),
)?;
```

**Step 4: Implement `CheckpointStore` for `FjallStore`**

In `crates/nexus-fjall/src/store.rs`:

```rust
impl CheckpointStore for FjallStore {
    type Error = FjallError;

    async fn load(&self, subscription_id: &(impl Id + Sync)) -> Result<Option<Version>, FjallError> {
        let key = subscription_id.to_string();
        let Some(bytes) = self.checkpoints.get(&key)? else {
            return Ok(None);
        };
        // Value is u64 big-endian (8 bytes)
        let raw = u64::from_be_bytes(
            bytes.as_ref().try_into().map_err(|_| FjallError::CorruptValue {
                stream_id: key,
                version: None,
            })?
        );
        Version::new(raw).map_or_else(
            || Err(FjallError::CorruptValue {
                stream_id: subscription_id.to_string(),
                version: Some(raw),
            }),
            |v| Ok(Some(v)),
        )
    }

    async fn save(
        &self,
        subscription_id: &(impl Id + Sync),
        version: Version,
    ) -> Result<(), FjallError> {
        let key = subscription_id.to_string();
        let value = version.as_u64().to_be_bytes();
        self.checkpoints.insert(&key, value)?;
        Ok(())
    }
}
```

Note: `save` uses a simple `insert` (not a transaction) because it's a single-key write. If we later need atomic checkpoint+projection, that's #149.

**Step 5: Run tests**

Run: `cargo test -p nexus-fjall -- checkpoint`

Expected: All pass.

**Step 6: Commit**

```
feat(fjall): implement CheckpointStore for FjallStore (#125)
```

---

### Task 6: Full Integration Verification

**Step 1: Run the complete test suite**

Run: `cargo test --all`

Expected: All tests pass across all crates.

**Step 2: Run clippy across workspace**

Run: `cargo clippy --all-targets -- --deny warnings`

Expected: No warnings.

**Step 3: Run formatting**

Run: `cargo fmt --all --check`

Expected: No formatting issues.

**Step 4: Run hakari verification**

Run: `cargo hakari generate --diff && cargo hakari manage-deps --dry-run && cargo hakari verify`

If hakari reports differences, run `cargo hakari generate && cargo hakari manage-deps` and commit.

**Step 5: Commit any fixups**

If hakari changes were needed:

```
chore: update workspace-hack for tokio sync dependency
```

---

### Task 7: Update Exports and Documentation

**Files:**
- Modify: `crates/nexus-fjall/src/lib.rs` (ensure all new public types are exported)
- Modify: `crates/nexus-store/src/lib.rs` (ensure trait re-exports are complete)

**Step 1: Verify public API surface**

Check that all new public types are reachable from crate roots:

- `nexus_store::Subscription`
- `nexus_store::CheckpointStore`
- `nexus_fjall::FjallSubscriptionStream`

**Step 2: Run `cargo doc --all --no-deps`**

Expected: No warnings. All public items have doc comments.

**Step 3: Commit**

```
docs: ensure subscription types are exported and documented (#125)
```

---

### Summary of Files Changed

| File | Action | Task |
|------|--------|------|
| `crates/nexus-store/src/store/subscription.rs` | Create | 1 |
| `crates/nexus-store/src/store/checkpoint.rs` | Create | 1 |
| `crates/nexus-store/src/store/mod.rs` | Modify | 1 |
| `crates/nexus-store/src/lib.rs` | Modify | 1 |
| `crates/nexus-store/src/testing.rs` | Modify | 2 |
| `crates/nexus-store/tests/subscription_tests.rs` | Create | 2 |
| `crates/nexus-fjall/Cargo.toml` | Modify | 3 |
| `crates/nexus-fjall/src/store.rs` | Modify | 3, 4, 5 |
| `crates/nexus-fjall/src/builder.rs` | Modify | 3, 5 |
| `crates/nexus-fjall/src/subscription_stream.rs` | Create | 4 |
| `crates/nexus-fjall/src/lib.rs` | Modify | 4 |
| `crates/nexus-fjall/src/encoding.rs` | Modify (maybe) | 5 |
| `crates/nexus-fjall/tests/subscription_tests.rs` | Create | 4, 5 |
