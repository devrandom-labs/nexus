# Bounded Batch Size Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cap how many event rows `read_stream` and subscription refills hold in memory at once, with a compile-time ceiling and a runtime-configurable batch size, so a slow consumer or large replay cannot OOM the process.

**Architecture:** A validated `BatchSize` newtype (`1..=MAX_BATCH`) lives in `nexus-store`. Each adapter's read cursor loads at most `batch_size` rows, then refills the next batch by keyset resume (`last_version + 1`) until a short batch signals end-of-data (one-shot read → `None`; subscription → park on the wake registry). This works without transactional snapshots because the event store is append-only with gapless per-stream versions.

**Tech Stack:** Rust 2024, `fjall` 3.1.4 (lazy `range().take(n)`), `futures::Stream` / `stream::unfold`, `tokio` (in-memory test store), `thiserror`, `proptest`. Build/test via `nix develop -c cargo …`.

**Spec:** `docs/plans/2026-06-16-bounded-batch-size-design.md`

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/nexus-store/src/batch.rs` | `BatchSize` newtype, `MAX_BATCH`/`DEFAULT_BATCH`, `BatchSizeError` | **create** |
| `crates/nexus-store/src/lib.rs` | module decl + re-export | modify |
| `crates/nexus-store/src/store.rs` | doc the refill chunking on `read_stream` | modify (docs) |
| `crates/nexus-store/src/subscription.rs` | doc bounded catch-up on `subscribe` | modify (docs) |
| `crates/nexus-store/src/testing.rs` | `InMemoryStore` batch config, Arc-wrap map, paginating `InMemoryStream`, bounded subscription refill | modify |
| `crates/nexus-fjall/src/error.rs` | host `pub(crate) reason_label` (moved from store.rs) | modify |
| `crates/nexus-fjall/src/store.rs` | `batch_size` field, bounded `read_stream` | modify |
| `crates/nexus-fjall/src/stream.rs` | extract `decode_row`, paginating `FjallStream` | modify |
| `crates/nexus-fjall/src/builder.rs` | `batch_size(BatchSize)` setter | modify |

---

## Task 1: `BatchSize` newtype + ceiling const (nexus-store)

**Files:**
- Create: `crates/nexus-store/src/batch.rs`
- Modify: `crates/nexus-store/src/lib.rs:83` (module decls) and `:130` area (re-exports)

- [ ] **Step 1: Write the failing tests**

Create `crates/nexus-store/src/batch.rs` with the test module only at first is not allowed (the SUT must exist to compile). Instead create the full file in Step 3; here, write the tests appended to the same file. To keep TDD honest, write the test module now and the impl in Step 3 — but the file must compile, so write both in Step 3 and run the test first there. Practically: create the file in Step 3, then run tests. (This task's "fail" gate is Step 2's compile error before the file exists.)

Test code to include in `crates/nexus-store/src/batch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{BatchSize, BatchSizeError, DEFAULT_BATCH, MAX_BATCH};

    #[test]
    fn rejects_zero() {
        let err = BatchSize::new(0).unwrap_err();
        assert_eq!(err, BatchSizeError { requested: 0, max: MAX_BATCH });
    }

    #[test]
    fn rejects_above_max() {
        let err = BatchSize::new(MAX_BATCH + 1).unwrap_err();
        assert_eq!(err.requested, MAX_BATCH + 1);
        assert_eq!(err.max, MAX_BATCH);
    }

    #[test]
    fn accepts_lower_and_upper_bound() {
        assert_eq!(BatchSize::new(1).expect("1 is valid").get(), 1);
        assert_eq!(BatchSize::new(MAX_BATCH).expect("MAX is valid").get(), MAX_BATCH);
    }

    #[test]
    fn default_is_default_batch() {
        assert_eq!(BatchSize::DEFAULT.get(), DEFAULT_BATCH);
        assert_eq!(BatchSize::default().get(), DEFAULT_BATCH);
    }
}
```

- [ ] **Step 2: Verify it fails to compile**

Run: `nix develop -c cargo build -p nexus-store`
Expected: FAIL — `file not found for module batch` (until Step 3 adds the module decl) / `cannot find type BatchSize`.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/nexus-store/src/batch.rs` (above the test module):

```rust
//! Bounded batch size for stream reads and subscription refills.
//!
//! Read paths never materialize more than `batch_size` event rows at once.
//! [`BatchSize`] carries the `1..=MAX_BATCH` invariant by construction, so an
//! out-of-range value is unrepresentable past [`BatchSize::new`] — the builder
//! that accepts a `BatchSize` cannot be handed an invalid one.

use thiserror::Error;

/// Largest permitted batch size — the compile-time ceiling on resident rows.
pub const MAX_BATCH: usize = 4096;

/// Default batch size when none is configured.
pub const DEFAULT_BATCH: usize = 256;

/// A validated read / refill batch size in `1..=MAX_BATCH`.
///
/// Construct via [`BatchSize::new`]; the invalid range is rejected there, so
/// every `BatchSize` value in the program is in range by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchSize(usize);

impl BatchSize {
    /// The default batch size ([`DEFAULT_BATCH`]).
    pub const DEFAULT: Self = Self(DEFAULT_BATCH);

    /// Construct a `BatchSize`, rejecting `0` and values above [`MAX_BATCH`].
    ///
    /// # Errors
    ///
    /// Returns [`BatchSizeError`] if `n == 0` or `n > MAX_BATCH`.
    pub const fn new(n: usize) -> Result<Self, BatchSizeError> {
        if n == 0 || n > MAX_BATCH {
            return Err(BatchSizeError {
                requested: n,
                max: MAX_BATCH,
            });
        }
        Ok(Self(n))
    }

    /// The validated value, always in `1..=MAX_BATCH`.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Error returned when a configured batch size is out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("batch size must be in 1..={max}, got {requested}")]
pub struct BatchSizeError {
    /// The rejected value.
    pub requested: usize,
    /// The compile-time maximum ([`MAX_BATCH`]).
    pub max: usize,
}
```

Add the module decl in `crates/nexus-store/src/lib.rs` next to the other `pub mod` lines (alphabetically near `pub mod builder;` at line 83):

```rust
pub mod batch;
```

Add the re-export near `pub use store::{GlobalSeq, RawEventStore, Store};` (line 130):

```rust
pub use batch::{BatchSize, BatchSizeError, DEFAULT_BATCH, MAX_BATCH};
```

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-store batch::`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/batch.rs crates/nexus-store/src/lib.rs
git commit -m "feat(store): BatchSize newtype with MAX_BATCH ceiling (#176)"
```

(The pre-commit hook runs `nix flake check`; let it complete.)

---

## Task 2: In-memory store — batch config plumbing (nexus-store)

Add the configurable batch size and Arc-wrap the map so the read cursor can refill. No behavior change yet (read still returns everything — bounded in Task 3).

**Files:**
- Modify: `crates/nexus-store/src/testing.rs:94-110` (struct + constructors), `:244` (append lock), `:315` (read_stream lock)

- [ ] **Step 1: Write the failing test**

Append to the `tests` module of `crates/nexus-store/src/testing.rs` (create the module section if needed — see existing test helpers in that file; if none, add `#[cfg(test)] mod tests { use super::*; ... }` at the end):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod batch_config_tests {
    use super::*;
    use crate::batch::{BatchSize, DEFAULT_BATCH};

    #[test]
    fn default_store_uses_default_batch() {
        let store = InMemoryStore::new();
        assert_eq!(store.batch_size().get(), DEFAULT_BATCH);
    }

    #[test]
    fn with_batch_size_overrides_default() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(8).unwrap());
        assert_eq!(store.batch_size().get(), 8);
    }
}
```

- [ ] **Step 2: Verify it fails**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing batch_config`
Expected: FAIL — `no method named batch_size` / `no function with_batch_size`.

- [ ] **Step 3: Write the implementation**

In `crates/nexus-store/src/testing.rs`, update imports at the top (after the existing `use` block):

```rust
use crate::batch::BatchSize;
```

Change the struct (lines 94-98) to Arc-wrap the map and carry the batch size:

```rust
pub struct InMemoryStore {
    streams: Arc<Mutex<HashMap<String, Vec<StoredFrame>>>>,
    notifiers: Arc<StreamNotifiers>,
    next_global_seq: Mutex<GlobalSeq>,
    batch_size: BatchSize,
}
```

Replace `new` and add `with_batch_size` + `batch_size` accessor (lines 100-110):

```rust
impl InMemoryStore {
    /// Create a new empty in-memory store with the default batch size.
    #[must_use]
    pub fn new() -> Self {
        Self::with_batch_size(BatchSize::DEFAULT)
    }

    /// Create a new empty in-memory store with an explicit batch size.
    #[must_use]
    pub fn with_batch_size(batch_size: BatchSize) -> Self {
        Self {
            streams: Arc::new(Mutex::new(HashMap::new())),
            notifiers: StreamNotifiers::new(),
            next_global_seq: Mutex::new(GlobalSeq::INITIAL),
            batch_size,
        }
    }

    /// The configured read / refill batch size.
    #[must_use]
    pub const fn batch_size(&self) -> BatchSize {
        self.batch_size
    }
}
```

The `append` and `read_stream` bodies already call `self.streams.lock().await`; `Arc<Mutex<…>>` derefs to `Mutex`, so those lines are unchanged.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription`
Expected: PASS — new batch_config tests plus all existing in-memory tests still green.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): configurable batch size on InMemoryStore (#176)"
```

---

## Task 3: In-memory store — bounded, paginating `read_stream`

Replace the all-rows `InMemoryStream` with an unfold cursor that loads `batch_size` rows at a time and keyset-resumes until a short batch ends it.

**Files:**
- Modify: `crates/nexus-store/src/testing.rs:118-153` (`InMemoryStream`), `:315-335` (`read_stream`)
- Add: free fn `scan_batch` (shared by read + subscription refill)

- [ ] **Step 1: Write the failing tests**

Append to `crates/nexus-store/src/testing.rs` test section:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod bounded_read_tests {
    use super::*;
    use crate::batch::BatchSize;
    use crate::envelope::pending_envelope;
    use futures::StreamExt;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    fn env(v: u64) -> PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E")
            .payload(vec![v as u8])
            .unwrap()
            .build()
    }

    async fn seed(store: &InMemoryStore, id: &Tid, count: u64) {
        // append one at a time to exercise sequential versions
        for v in 1..=count {
            let expected = Version::new(v - 1);
            store.append(id, expected, &[env(v)]).await.unwrap();
        }
    }

    #[tokio::test]
    async fn read_yields_all_events_across_refills() {
        // batch_size 4, 14 events => 4 refills (4+4+4+2)
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 14).await;

        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = stream.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=14).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn read_terminates_at_exact_batch_boundary() {
        // exactly batch_size events: must terminate, not hang or double-yield
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 4).await;

        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let seen: Vec<u64> = {
            let mut v = Vec::new();
            while let Some(item) = stream.next().await {
                v.push(item.unwrap().version().as_u64());
            }
            v
        };
        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn read_empty_stream_yields_nothing() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(4).unwrap());
        let id = Tid("missing".into());
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_from_midpoint_resumes_correctly() {
        let store = InMemoryStore::with_batch_size(BatchSize::new(3).unwrap());
        let id = Tid("s".into());
        seed(&store, &id, 10).await;
        let mut stream = store.read_stream(&id, Version::new(6).unwrap()).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = stream.next().await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, vec![6, 7, 8, 9, 10]);
    }
}
```

- [ ] **Step 2: Verify it fails**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription bounded_read`
Expected: FAIL — `read_yields_all_events_across_refills` returns all 14 today (no bound), but the test will only pass once `InMemoryStream` paginates; before the rewrite it still passes by accident (returns all). To make the test meaningful, it asserts ordering AND the boundary/midpoint cases. The boundary/midpoint tests pass trivially today; the real proof is Task 6's resident-memory assertion. Run and confirm current behavior, then proceed to rewrite.

> Note: these read tests pass against the current all-rows implementation too — they pin the *contract* (all events, in order, across refills). The bound itself is asserted in Task 6. This is intentional: we lock the observable contract here so the rewrite cannot regress it.

- [ ] **Step 3: Write the implementation**

In `crates/nexus-store/src/testing.rs`, add the shared scan helper (place near `frame_to_envelope`):

```rust
/// Collect up to `batch_size` frames with `version >= from`, in version order.
///
/// `rows` is in insertion = version order, so the matching frames are a
/// contiguous suffix; `take(batch_size)` bounds the materialized slice.
fn scan_batch(rows: &[StoredFrame], from: u64, batch_size: usize) -> VecDeque<StoredFrame> {
    rows.iter()
        .filter(|r| r.version >= from)
        .take(batch_size)
        .cloned()
        .collect()
}
```

Add `use std::collections::VecDeque;` to the top imports if not already present (the file uses `std::collections::VecDeque` inline today — add the import and drop the inline paths).

Replace the `InMemoryStream` struct and its `Stream` impl (lines 118-153) with an unfold-backed cursor plus its read state:

```rust
/// Keyset-paginating read state for a one-shot `read_stream`.
struct ReadState {
    streams: Arc<Mutex<HashMap<String, Vec<StoredFrame>>>>,
    stream_id: String,
    /// Start version of the *first* batch (used until the first yield).
    from: Version,
    last_version: Option<Version>,
    batch_size: usize,
    buffer: VecDeque<StoredFrame>,
    /// Set when a refill returns fewer than `batch_size` rows — no more data.
    done: bool,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl ReadState {
    fn next_read_version(&self) -> Result<Version, InMemoryStoreError> {
        self.last_version.map_or(Ok(self.from), |v| {
            v.next().ok_or(InMemoryStoreError::VersionOverflow)
        })
    }

    async fn refill(&mut self, from: Version) {
        let batch = {
            let guard = self.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| scan_batch(rows, from.as_u64(), self.batch_size))
                .unwrap_or_default()
        };
        self.done = batch.len() < self.batch_size;
        self.buffer = batch;
    }
}

/// `futures::Stream` of envelopes over an in-memory stream.
///
/// Loads at most `batch_size` rows at a time and keyset-resumes
/// (`last_version + 1`) when the buffer drains, terminating with `None` once a
/// refill returns a short (or empty) batch.
pub struct InMemoryStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, InMemoryStoreError>> + Send>,
    >,
}

impl futures::Stream for InMemoryStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
```

Replace `read_stream` (lines 315-335) to build the unfold cursor:

```rust
    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        let state = ReadState {
            streams: Arc::clone(&self.streams),
            stream_id: id.to_string(),
            from,
            last_version: None,
            batch_size: self.batch_size.get(),
            buffer: VecDeque::new(),
            done: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some(row) = s.buffer.pop_front() {
                    #[cfg(debug_assertions)]
                    {
                        if let Some(prev) = s.prev_version {
                            debug_assert!(
                                row.version > prev,
                                "InMemoryStream monotonicity violated: version {} not > previous {}",
                                row.version,
                                prev,
                            );
                        }
                        s.prev_version = Some(row.version);
                    }
                    return match frame_to_envelope(&row) {
                        Ok(env) => {
                            s.last_version = Some(env.version());
                            Some((Ok(env), s))
                        }
                        Err(e) => Some((Err(e), s)),
                    };
                }
                if s.done {
                    return None;
                }
                let from = match s.next_read_version() {
                    Ok(v) => v,
                    Err(e) => return Some((Err(e), s)),
                };
                s.refill(from).await;
                if s.buffer.is_empty() {
                    // refill returned nothing => end of stream
                    return None;
                }
            }
        });

        Ok(InMemoryStream {
            inner: Box::pin(unfolded),
        })
    }
```

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription`
Expected: PASS — `bounded_read_tests` (4) plus all existing in-memory tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): bounded paginating read_stream on InMemoryStore (#176)"
```

---

## Task 4: In-memory store — bounded subscription refill

Bound the in-memory subscription cursor's refill to `batch_size` and thread the limit through `SubState`.

**Files:**
- Modify: `crates/nexus-store/src/testing.rs:351-387` (`SubState`), `:417-444` (`subscribe`)

- [ ] **Step 1: Write the failing test**

Append to the test section:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod bounded_subscription_tests {
    use super::*;
    use crate::batch::BatchSize;
    use crate::envelope::pending_envelope;
    use crate::subscription::RawSubscription;
    use futures::StreamExt;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    fn env(v: u64) -> PendingEnvelope {
        pending_envelope(Version::new(v).unwrap())
            .event_type("E")
            .payload(vec![v as u8])
            .unwrap()
            .build()
    }

    #[tokio::test]
    async fn subscription_drains_many_batches_then_sees_new_event() {
        // batch_size 4; pre-seed 10 (catch-up across 3 refills), then push 1 live.
        let store = Arc::new(InMemoryStore::with_batch_size(BatchSize::new(4).unwrap()));
        let id = Tid("s".into());
        for v in 1..=10 {
            store.append(&id, Version::new(v - 1), &[env(v)]).await.unwrap();
        }

        let mut sub = InMemoryStore::subscribe(&store, &id, None).await.unwrap();

        // Drain the 10 catch-up events (bounded internally to 4 at a time).
        for expected in 1..=10u64 {
            let got = sub.next().await.unwrap().unwrap();
            assert_eq!(got.version().as_u64(), expected);
        }

        // Append a live event; the subscription must surface it after waiting.
        let store2 = Arc::clone(&store);
        let id2 = id.clone();
        tokio::spawn(async move {
            store2.append(&id2, Version::new(10), &[env(11)]).await.unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.version().as_u64(), 11);
    }
}
```

- [ ] **Step 2: Verify it fails (or passes by accident)**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription bounded_subscription`
Expected: PASS today (unbounded refill still yields all). As with Task 3, this pins the contract; Task 6 proves the bound. Confirm green, then make the refill bounded so the contract holds under the new limit.

- [ ] **Step 3: Write the implementation**

In `crates/nexus-store/src/testing.rs`, add `batch_size` to `SubState` (lines 351-362):

```rust
struct SubState {
    store: Arc<InMemoryStore>,
    stream_id: String,
    buffer: std::collections::VecDeque<StoredFrame>,
    last_version: Option<Version>,
    batch_size: usize,
    guard: SubscriptionGuard,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}
```

Replace `SubState::refill` (lines 365-379) to use the shared bounded scan:

```rust
    async fn refill(&mut self, from_version: Version) {
        let buffer = {
            let guard = self.store.streams.lock().await;
            guard
                .get(&self.stream_id)
                .map(|rows| scan_batch(rows, from_version.as_u64(), self.batch_size))
                .unwrap_or_default()
        };
        self.buffer = buffer;
    }
```

In `subscribe` (lines 433-441), set `batch_size` when building `SubState`:

```rust
        let mut state = SubState {
            store: Arc::clone(arc),
            stream_id,
            buffer: std::collections::VecDeque::new(),
            last_version: from,
            batch_size: arc.batch_size.get(),
            guard,
            #[cfg(debug_assertions)]
            prev_version: from.map(Version::as_u64),
        };
```

The unfold loop is unchanged: it yields the buffer, refills (now bounded) on empty, and parks on `notified` only when a refill comes back empty. A full bounded batch leaves the buffer non-empty, so the cursor keeps draining without waiting.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription`
Expected: PASS — all in-memory + subscription tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): bound in-memory subscription refill to batch_size (#176)"
```

---

## Task 5: fjall — extract `decode_row`, move `reason_label`

Prepare the fjall stream for pagination by extracting the per-row decode (so unit tests don't need a live keyspace) and relocating `reason_label` so both `store.rs` and `stream.rs` can use it.

**Files:**
- Modify: `crates/nexus-fjall/src/error.rs` (add `pub(crate) reason_label`)
- Modify: `crates/nexus-fjall/src/store.rs:19-25` (remove local `reason_label`, import it)
- Modify: `crates/nexus-fjall/src/stream.rs:30-201` (extract `decode_row`, rewrite tests to call it)

- [ ] **Step 1: Write the failing test**

In `crates/nexus-fjall/src/stream.rs`, replace the existing `tests` module's two tests with decode-function tests:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::encoding::{encode_event_key, encode_event_value};

    fn row(id: &[u8], version: u64, global_seq: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_event_key(id, version).unwrap();
        let mut val = Vec::new();
        encode_event_value(&mut val, global_seq, 1, et, None, payload).unwrap();
        (Slice::from(key), Slice::from(val))
    }

    #[test]
    fn decode_row_yields_envelope() {
        let (k, v) = row(b"user-1", 7, 42, "Created", b"data");
        let sid = ArrayString::try_from("user-1").unwrap();
        let env = decode_row(&k, v, sid).unwrap();
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn decode_row_rejects_truncated_value() {
        let k = Slice::from(encode_event_key(b"corrupt", 1).unwrap());
        let v = Slice::from(&[0u8, 1, 2][..]);
        let sid = ArrayString::try_from("corrupt").unwrap();
        match decode_row(&k, v, sid).unwrap_err() {
            FjallError::CorruptValue { stream_id, version } => {
                assert_eq!(stream_id.as_str(), "corrupt");
                assert_eq!(version, Some(1));
            }
            other => panic!("expected CorruptValue, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Verify it fails**

Run: `nix develop -c cargo nextest run -p nexus-fjall stream::tests::decode_row`
Expected: FAIL — `cannot find function decode_row`.

- [ ] **Step 3: Write the implementation**

In `crates/nexus-fjall/src/error.rs`, add (after the imports, above the enum):

```rust
use std::fmt::Write;

/// Format a `Display` value into a stack-allocated reason label,
/// silently truncating at 128 bytes on a char boundary.
pub(crate) fn reason_label(value: &impl std::fmt::Display) -> ArrayString<128> {
    let mut buf = ArrayString::<128>::new();
    let _ = write!(buf, "{value}");
    buf
}
```

In `crates/nexus-fjall/src/store.rs`, delete the local `reason_label` (lines 19-25) and the now-unused `use std::fmt::Write;` (line 15), and import the shared one — add to the top `use` block:

```rust
use crate::error::{reason_label, FjallError};
```

(Replace the existing `use crate::error::FjallError;` line.)

In `crates/nexus-fjall/src/stream.rs`, extract the decode into a free function (place above `impl FjallStream`):

```rust
/// Decode one stored `(key, value)` row into a [`PersistedEnvelope`].
///
/// Pure: no store handle, no cursor state — so unit tests can exercise it
/// with hand-built rows. Maps every malformed shape to a typed `FjallError`.
fn decode_row(
    key: &Slice,
    value: Slice,
    stream_id: ArrayString<64>,
) -> Result<PersistedEnvelope, FjallError> {
    let (_id_bytes, version) = decode_event_key(key).map_err(|_| FjallError::CorruptValue {
        stream_id,
        version: None,
    })?;

    let bytes_value: Bytes = value.into();
    let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    let ver = Version::new(version).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    let global_seq = GlobalSeq::new(decoded.global_seq).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    PersistedEnvelope::try_new(
        ver,
        global_seq,
        bytes_value,
        decoded.schema_version,
        decoded.event_type_range,
        decoded.payload_range,
        decoded.metadata_range,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id,
        version,
        source,
    })
}
```

Rewrite the existing `poll_one` (lines 31-101) to delegate to `decode_row`, keeping the poison + debug-monotonicity behavior (the full paginating version lands in Task 6 — for now keep the single-batch behavior so the crate compiles and existing store tests pass):

```rust
impl FjallStream {
    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        let (key, value) = self.events.pop_front()?;
        match decode_row(&key, value, self.stream_id) {
            Ok(env) => {
                #[cfg(debug_assertions)]
                {
                    let v = env.version().as_u64();
                    if let Some(prev) = self.prev_version {
                        debug_assert!(
                            v > prev,
                            "EventStream monotonicity violated: version {v} is not greater than previous {prev}",
                        );
                    }
                    self.prev_version = Some(v);
                }
                Some(Ok(env))
            }
            Err(e) => {
                self.poisoned = true;
                Some(Err(e))
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-fjall`
Expected: PASS — new `decode_row` tests plus all existing store/stream tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/error.rs crates/nexus-fjall/src/store.rs crates/nexus-fjall/src/stream.rs
git commit -m "refactor(fjall): extract decode_row, share reason_label (#176)"
```

---

## Task 6: fjall — paginating `FjallStream` + bounded `read_stream` + builder config

The core change: `FjallStream` holds a cloned `events` keyspace handle and refills itself via `range().take(batch_size)`; `read_stream` builds it bounded; the builder configures the size.

**Files:**
- Modify: `crates/nexus-fjall/src/stream.rs` (struct fields, constructor, paginating `poll_one`/`refill`)
- Modify: `crates/nexus-fjall/src/store.rs` (`batch_size` field, simplified bounded `read_stream`)
- Modify: `crates/nexus-fjall/src/builder.rs` (`batch_size` field + setter, thread into `open`)

- [ ] **Step 1: Write the failing tests**

Add to `crates/nexus-fjall/src/store.rs` `tests` module:

```rust
    use crate::batch_test_helpers::*;

    #[tokio::test]
    async fn read_yields_all_across_refills() {
        let (store, _dir) = store_with_batch(4);
        let id = tid("s");
        seed(&store, &id, 14).await;
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=14).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn read_terminates_at_exact_batch_boundary() {
        let (store, _dir) = store_with_batch(4);
        let id = tid("s");
        seed(&store, &id, 8).await; // exactly 2 full batches
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut count = 0u64;
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            item.unwrap();
            count += 1;
        }
        assert_eq!(count, 8);
    }

    #[tokio::test]
    async fn read_reopened_store_recovers_all_across_refills() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let id = tid("s");
        {
            let store = FjallStore::builder(&path)
                .batch_size(nexus_store::batch::BatchSize::new(4).unwrap())
                .open()
                .unwrap();
            seed(&store, &id, 10).await;
        }
        let store = FjallStore::builder(&path)
            .batch_size(nexus_store::batch::BatchSize::new(4).unwrap())
            .open()
            .unwrap();
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            seen.push(item.unwrap().version().as_u64());
        }
        assert_eq!(seen, (1..=10).collect::<Vec<_>>());
    }
```

Add a shared test helper module to `crates/nexus-fjall/src/store.rs` (outside the existing `tests` mod, gated on test):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
pub(crate) mod batch_test_helpers {
    use super::*;
    use nexus_store::batch::BatchSize;
    use nexus_store::envelope::pending_envelope;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    pub(crate) struct Tid(pub String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    pub(crate) fn tid(s: &str) -> Tid {
        Tid(s.to_owned())
    }

    pub(crate) fn store_with_batch(n: usize) -> (FjallStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db"))
            .batch_size(BatchSize::new(n).unwrap())
            .open()
            .unwrap();
        (store, dir)
    }

    pub(crate) async fn seed(store: &FjallStore, id: &Tid, count: u64) {
        for v in 1..=count {
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(vec![v as u8])
                .unwrap()
                .build();
            store.append(id, Version::new(v - 1), &[env]).await.unwrap();
        }
    }
}
```

- [ ] **Step 2: Verify it fails**

Run: `nix develop -c cargo nextest run -p nexus-fjall read_yields_all_across_refills`
Expected: FAIL — `no method named batch_size` on the builder.

- [ ] **Step 3: Write the implementation**

**`crates/nexus-fjall/src/builder.rs`** — add the field + setter and thread it into `open`.

Add import at top:

```rust
use nexus_store::batch::BatchSize;
```

Add `batch_size: BatchSize` to the struct (after `events_config`):

```rust
pub struct FjallStoreBuilder<S = (), E = ()> {
    path: PathBuf,
    streams_config: S,
    events_config: E,
    batch_size: BatchSize,
}
```

Set it in `new` (line 29-35) and carry it through the two `streams_config`/`events_config` builders (they reconstruct the struct — add `batch_size: self.batch_size` to each `FjallStoreBuilder { … }` literal at lines 49-53 and 66-70). In `new`:

```rust
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: (),
            events_config: (),
            batch_size: BatchSize::DEFAULT,
        }
```

Add the setter in the `impl<S, E> FjallStoreBuilder<S, E>` block:

```rust
    /// Set the read / refill batch size — the maximum number of event rows
    /// held in memory at once by a read stream or subscription refill.
    ///
    /// Out-of-range values are rejected when constructing the
    /// [`BatchSize`](nexus_store::batch::BatchSize), so this setter is
    /// infallible. Defaults to [`BatchSize::DEFAULT`].
    #[must_use]
    pub fn batch_size(mut self, batch_size: BatchSize) -> Self {
        self.batch_size = batch_size;
        self
    }
```

In `open` (line 98-107), pass it into the struct:

```rust
        Ok(FjallStore {
            db,
            streams,
            events,
            #[cfg(feature = "snapshot")]
            snapshots,
            global,
            notifiers: StreamNotifiers::new(),
            batch_size: self.batch_size,
        })
```

**`crates/nexus-fjall/src/store.rs`** — add the field and rewrite `read_stream`.

Add imports (top `use` block):

```rust
use nexus_store::batch::BatchSize;
```

Add the field to `FjallStore` (after `notifiers`, line 52):

```rust
    /// Maximum event rows materialized per read batch / refill.
    pub(crate) batch_size: BatchSize,
```

Replace `read_stream` (lines 221-278) entirely:

```rust
    async fn read_stream(&self, id: &impl Id, from: Version) -> Result<Self::Stream, Self::Error> {
        // A single bounded range scan; a nonexistent stream simply yields an
        // empty first batch (done), so no separate existence check is needed.
        FjallStream::new(
            self.events.clone(),
            OwnedStreamId::from_id(id),
            from.as_u64(),
            self.batch_size.get(),
            id.to_label(),
        )
    }
```

**`crates/nexus-fjall/src/stream.rs`** — paginating cursor.

Update imports at top:

```rust
use fjall::{Readable, Slice, SingleWriterTxKeyspace};
use crate::encoding::{decode_event_key, decode_event_value, encode_event_key};
use crate::error::{reason_label, FjallError};
use crate::subscription_stream::OwnedStreamId;
```

(Keep existing `use` lines for `VecDeque`, `ArrayString`, `Bytes`, `Version`, `GlobalSeq`, `PersistedEnvelope`. `Readable` brings `Keyspace::range` into scope for `self.keyspace.inner().range(...)`.)

Replace the `FjallStream` struct (lines 19-28):

```rust
pub struct FjallStream {
    /// Current in-memory batch, drained front-to-back.
    events: VecDeque<(Slice, Slice)>,
    /// Cloned `events` partition handle — Arc-backed, cheap to hold; used to
    /// scan the next batch when `events` drains.
    keyspace: SingleWriterTxKeyspace,
    /// Stream key bytes for refill key encoding.
    id: OwnedStreamId,
    /// Next version to scan from on the following refill (keyset cursor).
    next_version: u64,
    /// Max rows per batch.
    batch_size: usize,
    /// Set once a refill returns fewer than `batch_size` rows — no more data.
    done: bool,
    stream_id: ArrayString<64>,
    /// Once an error is yielded, the stream is poisoned — subsequent polls
    /// return `None` rather than silently skipping corrupt entries.
    poisoned: bool,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}
```

Add the constructor + refill, and make `poll_one` paginate (replace the `impl FjallStream` block written in Task 5):

```rust
impl FjallStream {
    /// Build a stream and eagerly load the first batch (so the first poll is
    /// IO-free), scanning from `from`.
    pub(crate) fn new(
        keyspace: SingleWriterTxKeyspace,
        id: OwnedStreamId,
        from: u64,
        batch_size: usize,
        stream_id: ArrayString<64>,
    ) -> Result<Self, FjallError> {
        let mut stream = Self {
            events: VecDeque::new(),
            keyspace,
            id,
            next_version: from,
            batch_size,
            done: false,
            stream_id,
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };
        stream.refill()?;
        Ok(stream)
    }

    /// Scan the next `batch_size` rows from `next_version` into `events`.
    /// A short batch (fewer than `batch_size`) sets `done`.
    fn refill(&mut self) -> Result<(), FjallError> {
        let id_bytes = self.id.as_ref();
        let start = encode_event_key(id_bytes, self.next_version).map_err(|e| {
            FjallError::InvalidInput {
                stream_id: self.stream_id,
                version: self.next_version,
                reason: reason_label(&e),
            }
        })?;
        let end = encode_event_key(id_bytes, u64::MAX).map_err(|e| FjallError::InvalidInput {
            stream_id: self.stream_id,
            version: u64::MAX,
            reason: reason_label(&e),
        })?;

        let batch: Vec<(Slice, Slice)> = self
            .keyspace
            .inner()
            .range(start..=end)
            .take(self.batch_size)
            .map(fjall::Guard::into_inner)
            .collect::<Result<_, _>>()?;

        self.done = batch.len() < self.batch_size;
        self.events = batch.into();
        Ok(())
    }

    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        loop {
            if let Some((key, value)) = self.events.pop_front() {
                return Some(match decode_row(&key, value, self.stream_id) {
                    Ok(env) => {
                        let v = env.version().as_u64();
                        #[cfg(debug_assertions)]
                        {
                            if let Some(prev) = self.prev_version {
                                debug_assert!(
                                    v > prev,
                                    "EventStream monotonicity violated: version {v} is not greater than previous {prev}",
                                );
                            }
                            self.prev_version = Some(v);
                        }
                        // Advance keyset cursor. No event can follow u64::MAX.
                        self.next_version = match v.checked_add(1) {
                            Some(n) => n,
                            None => {
                                self.done = true;
                                v
                            }
                        };
                        Ok(env)
                    }
                    Err(e) => {
                        self.poisoned = true;
                        Err(e)
                    }
                });
            }
            if self.done {
                return None;
            }
            if let Err(e) = self.refill() {
                self.poisoned = true;
                return Some(Err(e));
            }
            if self.events.is_empty() {
                // refill returned nothing => end of stream
                return None;
            }
        }
    }
}
```

> Note: `poll_one`'s refill is a synchronous fjall range scan — consistent with the pre-existing design (the old `read_stream` also scanned synchronously inside the async fn). Each scan is now bounded to `batch_size`, so no scan blocks longer than one batch.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo nextest run -p nexus-fjall`
Expected: PASS — the three new read tests, the `decode_row` tests, and all existing append/read/version tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/stream.rs crates/nexus-fjall/src/store.rs crates/nexus-fjall/src/builder.rs
git commit -m "feat(fjall): paginating FjallStream + bounded read_stream + batch_size builder (#176)"
```

---

## Task 7: fjall — subscription bounded across refills (test)

The fjall subscription drives a `FjallStream` (now paginating) and re-reads via `read_stream` (now bounded), so the bound propagates automatically. Prove it.

**Files:**
- Modify: `crates/nexus-fjall/src/subscription_stream.rs` (add a `tests` module)

- [ ] **Step 1: Write the failing test**

Add to `crates/nexus-fjall/src/subscription_stream.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use crate::store::batch_test_helpers::{seed, store_with_batch, tid};
    use futures::StreamExt;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::subscription::RawSubscription;

    #[tokio::test]
    async fn subscription_drains_many_batches_then_sees_live_event() {
        let (store, _dir) = store_with_batch(4);
        let store = Arc::new(store);
        let id = tid("s");
        seed(&store, &id, 10).await; // 3 bounded catch-up refills (4+4+2)

        let mut sub = FjallStore::subscribe(&store, &id, None).await.unwrap();
        for expected in 1..=10u64 {
            let got = sub.next().await.unwrap().unwrap();
            assert_eq!(got.version().as_u64(), expected);
        }

        let store2 = Arc::clone(&store);
        let id2 = id.clone();
        tokio::spawn(async move {
            let env = pending_envelope(Version::new(11).unwrap())
                .event_type("E")
                .payload(vec![11u8])
                .unwrap()
                .build();
            store2.append(&id2, Version::new(10), &[env]).await.unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.version().as_u64(), 11);
    }
}
```

- [ ] **Step 2: Verify it fails or passes**

Run: `nix develop -c cargo nextest run -p nexus-fjall subscription_stream::tests`
Expected: PASS (bound propagates from Task 6). If it hangs or yields out of order, the pagination/keyset logic in Task 6 is wrong — fix there.

- [ ] **Step 3: (no implementation change expected)**

If Step 2 passes, no code change. If `batch_test_helpers` isn't visible from `subscription_stream.rs`, confirm `pub(crate) mod batch_test_helpers` in `store.rs` (Task 6) and that it's `#[cfg(test)]`.

- [ ] **Step 4: Re-run**

Run: `nix develop -c cargo nextest run -p nexus-fjall`
Expected: PASS — whole crate.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/subscription_stream.rs
git commit -m "test(fjall): subscription stays bounded across catch-up refills (#176)"
```

---

## Task 8: Linearizability + property tests

Cross-cutting category 4 (concurrent reader/writer) and a property test over random `(stream_len, batch_size)`.

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` tests (concurrency)
- Create: `crates/nexus-store/tests/bounded_batch_proptest.rs` (property)

- [ ] **Step 1: Write the failing tests**

Concurrency test — add to `crates/nexus-fjall/src/store.rs` `tests` module:

```rust
    #[tokio::test]
    async fn reader_sees_monotonic_gapfree_while_writer_appends() {
        use std::sync::Arc as StdArc;
        let (store, _dir) = store_with_batch(4);
        let store = StdArc::new(store);
        let id = tid("s");
        seed(&store, &id, 4).await;

        let writer = StdArc::clone(&store);
        let wid = id.clone();
        let handle = tokio::spawn(async move {
            for v in 5..=20u64 {
                let env = nexus_store::envelope::pending_envelope(Version::new(v).unwrap())
                    .event_type("E")
                    .payload(vec![v as u8])
                    .unwrap()
                    .build();
                writer.append(&wid, Version::new(v - 1), &[env]).await.unwrap();
            }
        });

        // Reader paginates the snapshot it observes; assert strict monotonic,
        // gap-free version sequence (immutability guarantee).
        let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
        let mut prev = 0u64;
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            let v = item.unwrap().version().as_u64();
            assert_eq!(v, prev + 1, "version gap or reorder: {prev} -> {v}");
            prev = v;
        }
        handle.await.unwrap();
        assert!(prev >= 4, "reader saw at least the seeded events");
    }
```

Property test — create `crates/nexus-store/tests/bounded_batch_proptest.rs`:

```rust
//! Property: a bounded paginating read yields exactly the stream's events,
//! in order, regardless of how `batch_size` chunks them.
#![allow(clippy::unwrap_used)]

use futures::StreamExt;
use nexus_store::batch::BatchSize;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use nexus_store::Version;
use proptest::prelude::*;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct Tid(String);
impl std::fmt::Display for Tid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl AsRef<[u8]> for Tid {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl nexus::Id for Tid {
    const BYTE_LEN: usize = 0;
}

fn batch_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![Just(1usize), Just(2), Just(255), Just(256), 1usize..512]
}

fn len_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0u64), Just(1), Just(255), Just(256), Just(257), 0u64..600]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn read_yields_all_in_order(batch in batch_strategy(), len in len_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::with_batch_size(BatchSize::new(batch).unwrap());
            let id = Tid("s".into());
            for v in 1..=len {
                let env = pending_envelope(Version::new(v).unwrap())
                    .event_type("E")
                    .payload(vec![(v % 256) as u8])
                    .unwrap()
                    .build();
                store.append(&id, Version::new(v - 1), &[env]).await.unwrap();
            }
            let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
            let mut seen = Vec::new();
            while let Some(item) = stream.next().await {
                seen.push(item.unwrap().version().as_u64());
            }
            prop_assert_eq!(seen, (1..=len).collect::<Vec<_>>());
            Ok(())
        })?;
    }
}
```

- [ ] **Step 2: Verify they fail / run**

Run: `nix develop -c cargo nextest run -p nexus-fjall reader_sees_monotonic`
Run: `nix develop -c cargo nextest run -p nexus-store --features testing,subscription --test bounded_batch_proptest`
Expected: PASS if Tasks 3 & 6 are correct. A reorder/gap failure points back to the keyset logic.

- [ ] **Step 3: (fix only if red)** Adjust pagination logic in the relevant cursor; do not weaken the assertions.

- [ ] **Step 4: Re-run both**

Run: `nix develop -c cargo nextest run -p nexus-fjall && nix develop -c cargo nextest run -p nexus-store --features testing,subscription`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/store.rs crates/nexus-store/tests/bounded_batch_proptest.rs
git commit -m "test: linearizability + property coverage for bounded batches (#176)"
```

---

## Task 9: Documentation + final verification

**Files:**
- Modify: `crates/nexus-store/src/store.rs` (`read_stream` doc, lines 159-167)
- Modify: `crates/nexus-store/src/subscription.rs` (`subscribe` doc, lines 119-129)

- [ ] **Step 1: Document refill chunking on `RawEventStore::read_stream`**

In `crates/nexus-store/src/store.rs`, expand the `read_stream` doc comment (above line 163):

```rust
    /// Open a stream of events.
    ///
    /// Events are yielded one at a time as a `futures::Stream` of owned
    /// [`PersistedEnvelope`](crate::envelope::PersistedEnvelope)s.
    ///
    /// # Batching
    ///
    /// The cursor materializes at most `batch_size` rows at a time (see the
    /// adapter's batch-size configuration) and refills the next batch
    /// internally as it drains, by keyset resume on the stream version. This
    /// is invisible to callers: `next()` has the same contract regardless of
    /// how the events are chunked, and the stream returns `None` once the
    /// persisted stream is exhausted.
```

- [ ] **Step 2: Document bounded catch-up on `Subscription::subscribe`**

In `crates/nexus-store/src/subscription.rs`, add to the `subscribe` doc (around line 122):

```rust
    /// Catch-up reads are bounded: the cursor materializes at most the
    /// adapter's configured `batch_size` events per refill, paginating by
    /// keyset resume until caught up, then waits for new events. A slow
    /// consumer never forces the whole backlog into memory at once.
```

- [ ] **Step 3: Full check**

Run: `nix develop -c cargo nextest run --workspace --all-features`
Expected: PASS — whole workspace.

Run: `nix flake check`
Expected: PASS — clippy (deny), fmt, taplo, audit, deny, nextest, doc.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/store.rs crates/nexus-store/src/subscription.rs
git commit -m "docs(store): document bounded refill on read_stream + subscribe (#176)"
```

- [ ] **Step 5: Open the PR**

```bash
git push -u origin feat/bounded-batch-size
gh pr create --title "feat: bound subscription/read batch size (#176)" \
  --body "Closes #176. Adds BatchSize (1..=MAX_BATCH=4096, default 256), bounded paginating read_stream + subscription refills on both InMemoryStore and FjallStore. See docs/plans/2026-06-16-bounded-batch-size-{design,implementation}.md."
```

---

## Self-Review

**Spec coverage:**
- AC "fjall builder API for batch size, compile-time max" → Task 1 (`MAX_BATCH`) + Task 6 (`batch_size` setter). ✓
- AC "same for `InMemoryStore`" → Task 2 (`with_batch_size`). ✓
- AC "`FjallStream`/`InMemoryStream` refill when buffer drains" → Task 6 (fjall) + Task 3 (in-memory). ✓
- AC "bound honored by `read_stream` and both subscription refills" → Tasks 3, 4, 6, 7. ✓
- AC "subscription falls behind, 10× batch, drains across refills" → Task 4 + Task 7. ✓
- AC "one-shot read of > batch_size yields all in order" → Tasks 3, 6, 8. ✓
- AC "batch_size > max rejected at builder" → Task 1 (`BatchSize::new` rejects; builders accept only `BatchSize`). ✓
- AC "doc on refill semantics" → Task 9. ✓
- Spec 4 test categories: sequence (3, 6), lifecycle (6 reopen), defensive boundary (1, 3, 6 exact-boundary), linearizability (8). ✓

**Placeholder scan:** No "TBD"/"add error handling"-style gaps; every code step shows full code. (Task 1 Step 1/2 explains the create-then-test ordering explicitly rather than leaving it implicit.)

**Type consistency:** `BatchSize::new` / `BatchSize::DEFAULT` / `BatchSize::get` used consistently; `scan_batch(rows, from: u64, batch_size)` signature identical at all call sites; `decode_row(&Slice, Slice, ArrayString<64>)` consistent between definition (Task 5) and call (Task 6); `FjallStream::new(keyspace, id, from, batch_size, stream_id)` consistent between definition (Task 6) and `read_stream` call (Task 6); `batch_test_helpers` exported `pub(crate)` and consumed in Tasks 6/7/8.
