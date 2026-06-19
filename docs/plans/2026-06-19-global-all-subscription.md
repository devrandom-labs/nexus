# Global `$all` Subscription Cursor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an all-streams subscription cursor that yields every event across all streams in `GlobalSeq` order, resuming from a `GlobalSeq` position, never terminating when caught up — the store-wide sibling of the per-stream `Subscription::subscribe`.

**Architecture:** A second, `GlobalSeq`-ordered index of every event (because an ordered KV store sorts by exactly one key, and the existing `events` partition is keyed by `[id][version]`). The index is written in the *same append transaction* as the primary event row, so it is atomic by construction. A store-wide `Notify` (added to `StreamNotifiers`) wakes `$all` subscribers on every commit. Reads are a single bounded range scan over the index — full-row values flow zero-copy into `PersistedEnvelope`, gaps are tolerated for free (the cursor scans `> last`, never `last + 1`).

**Tech Stack:** Rust 2024, `fjall` 3 (`bytes_1`), `bytes::Bytes`, `tokio::sync::Notify`, `futures::Stream`/`unfold`. Closes #214.

---

## Background & Key Decisions (read first)

These were settled in design discussion on 2026-06-19:

1. **Full-row index, not a pointer index.** The `events_global` value is the *whole wire frame*, so a `$all` read is one range scan with zero-copy values — no N+1 point-reads back into `events`. Cost: each event's frame bytes are stored twice on disk (LZ4-compressed). Accepted.
2. **Written in the same append transaction** as the primary `events` row → atomic, no new atomicity surface (CLAUDE.md rule 1).
3. **`read_all` lives on `RawEventStore`** as a sibling of `read_stream` (a new `AllStream` associated type), so the global read is independently testable, not buried inside the subscription impl.
4. **Wakeups: a single store-wide `Notify`** added to `StreamNotifiers` (`wake_all`/`all_notifier`), woken on every commit. No drop-guard/refcount — it is always present. Every `$all` subscriber genuinely wants every event, so there is no thundering-herd regression.
5. **fjall key layout — version goes in the key.** The version is *not* in the wire-frame value (fjall keeps it in the `events` key). The `events_global` key is `[u64 BE global_seq][u64 BE version]`; the value is the identical frame `Slice` (shared zero-copy with the `events` insert). Prepending version to the value would break the 16-byte payload-alignment invariant — do **not** do that. The in-memory store has no issue: `StoredFrame` already carries `version`.
6. **Gap source:** fjall's counter is actually gapless today (read-modify-written inside the tx). The contract's "tolerate gaps" is for the future postgres adapter (SQL sequences keep values from rolled-back txns). The range-scan cursor handles both without caring.
7. **No migration / backfill.** The crate is unreleased; an empty `events_global` on an old store is a non-issue. No docs to update.

PR boundaries:
- **PR 1 (Phase 1):** read path — `events_global` partition + append write (fjall), `read_all` + `AllStream` on `RawEventStore`, both adapter impls, in-memory `BTreeMap` index. Mergeable & testable on its own.
- **PR 2 (Phase 2):** subscription — `wake_all` on `StreamNotifiers`, append wakes it, `RawAllSubscription` trait, `Subscription::subscribe_all`, both adapter cursor impls.

---

## File Structure

**Phase 1 (read path):**
- Modify `crates/nexus-store/src/store.rs` — add `AllStream` associated type + `read_all` to `RawEventStore`.
- Modify `crates/nexus-store/src/testing.rs` — `global_index: BTreeMap`, write it in `append`, `read_all` impl, `GlobalReadState`.
- Modify `crates/nexus-fjall/src/encoding.rs` — `encode_global_key` / `decode_global_key`.
- Modify `crates/nexus-fjall/src/store.rs` — `events_global` field, write in `append`, `read_all` impl.
- Modify `crates/nexus-fjall/src/builder.rs` — open the `events_global` partition.
- Create `crates/nexus-fjall/src/all_stream.rs` — `FjallAllStream` global-ordered read cursor.
- Modify `crates/nexus-fjall/src/lib.rs` — `mod all_stream;`.

**Phase 2 (subscription):**
- Modify `crates/nexus-store/src/notify.rs` — `all: Arc<Notify>` field, `wake_all`, `all_notifier`, hand-written `Default`.
- Modify `crates/nexus-store/src/subscription.rs` — `RawAllSubscription` trait, relax `Subscription::new` bound, add `subscribe_all`.
- Modify `crates/nexus-store/src/testing.rs` — call `wake_all` in `append`, `RawAllSubscription` impl + `InMemoryAllSubscriptionStream`.
- Modify `crates/nexus-fjall/src/store.rs` — call `wake_all` in `append`, `RawAllSubscription` impl.
- Create `crates/nexus-fjall/src/all_subscription_stream.rs` — `FjallAllSubscriptionStream` never-ending cursor.
- Modify `crates/nexus-fjall/src/lib.rs` — `mod all_subscription_stream;`.

---

## Conventions for every task

- Run the gate via the pre-commit hook (do **not** pre-run `nix flake check` by hand — memory: `feedback_dont_prerun_gate`). For fast local iteration use `nix develop -c cargo test -p <crate>` and `nix develop -c cargo clippy --all-targets` (flake clippy is `--lib` only — memory: `project_flake_clippy_lib_only`).
- `git add` any **new** source file before the gate runs (memory: `project_nix_flake_check_untracked`).
- Run `nix develop -c cargo fmt --all` before staging.
- Work on a feature branch; never commit to `main`. Use the `joeldsouzax` gh account. Squash-merge via `gh pr merge --squash --delete-branch`.

---

# PHASE 1 — Read Path (PR 1)

## Task 1: `read_all` + `AllStream` on `RawEventStore`

**Files:**
- Modify: `crates/nexus-store/src/store.rs:112-178` (the `RawEventStore` trait)

- [ ] **Step 1: Add the associated type and method to the trait**

In `crates/nexus-store/src/store.rs`, inside `pub trait RawEventStore`, after the existing `read_stream` method (around line 177), add the `AllStream` associated type next to `Stream` (after line 122) and the `read_all` method after `read_stream`:

Add after `type Stream: EventStream<Error = Self::Error> + 'static;`:

```rust
    /// The stream type for an all-streams (`$all`) read.
    ///
    /// Owned, non-GAT, `'static` — a `futures::Stream` of
    /// `Result<PersistedEnvelope, Self::Error>` ordered by [`GlobalSeq`],
    /// not by `(stream, version)`. Distinct from [`Stream`](Self::Stream)
    /// because the global order is a different physical index.
    type AllStream: EventStream<Error = Self::Error> + 'static;
```

Add after the `read_stream` method (after line 177):

```rust
    /// Open a one-shot read over **all** streams, ordered by [`GlobalSeq`].
    ///
    /// `from` is **inclusive**: the stream yields every event with
    /// `global_seq >= from`, in ascending `GlobalSeq` order, then terminates
    /// with `None`. The sequence is monotonic but **not** gapless — burned
    /// values are simply absent, and this read tolerates them by scanning a
    /// range rather than stepping `from + 1`.
    ///
    /// This is the building block under an all-streams subscription; the
    /// never-ending wait-when-caught-up behaviour is layered on top.
    ///
    /// # Batching
    ///
    /// Like [`read_stream`](Self::read_stream), an adapter materializes at
    /// most its configured `batch_size` rows at a time, keyset-resuming on
    /// `GlobalSeq`.
    fn read_all(
        &self,
        from: GlobalSeq,
    ) -> impl std::future::Future<Output = Result<Self::AllStream, Self::Error>> + Send;
```

- [ ] **Step 2: Verify it fails to compile (both adapters now miss the item)**

Run: `nix develop -c cargo build -p nexus-store -p nexus-fjall`
Expected: FAIL — `not all trait items implemented, missing: AllStream, read_all` for both `InMemoryStore` and `FjallStore`. (Implemented in Tasks 2 and 5.)

- [ ] **Step 3: Commit (trait only)**

```bash
git add crates/nexus-store/src/store.rs
git commit -m "feat(store): read_all + AllStream on RawEventStore (#214)"
```

---

## Task 2: In-memory global index + `read_all`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`
- Test: `crates/nexus-store/src/testing.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the test module at the bottom of `crates/nexus-store/src/testing.rs` (reuse the existing test helpers there — `TestId`/`tid` and `pending_envelope`; match the patterns already in that module's tests):

```rust
    #[tokio::test]
    async fn read_all_yields_global_order_across_streams() {
        use futures::StreamExt;
        let store = InMemoryStore::new();
        // Interleave appends across two streams: a@1, b@1, a@2.
        append_one(&store, "a", 1, None, b"a1").await;
        append_one(&store, "b", 1, None, b"b1").await;
        append_one(&store, "a", 2, Some(1), b"a2").await;

        let mut all = store.read_all(GlobalSeq::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(env) = all.next().await {
            let env = env.unwrap();
            seen.push((env.global_seq().as_u64(), env.payload().to_vec()));
        }
        assert_eq!(
            seen,
            vec![
                (1, b"a1".to_vec()),
                (2, b"b1".to_vec()),
                (3, b"a2".to_vec()),
            ],
            "read_all must yield every event across streams in GlobalSeq order"
        );
    }

    #[tokio::test]
    async fn read_all_from_is_inclusive_and_resumes() {
        use futures::StreamExt;
        let store = InMemoryStore::new();
        append_one(&store, "a", 1, None, b"a1").await;
        append_one(&store, "a", 2, Some(1), b"a2").await;
        append_one(&store, "a", 3, Some(2), b"a3").await;

        let mut all = store.read_all(GlobalSeq::new(2).unwrap()).await.unwrap();
        let mut seqs = Vec::new();
        while let Some(env) = all.next().await {
            seqs.push(env.unwrap().global_seq().as_u64());
        }
        assert_eq!(seqs, vec![2, 3], "from is inclusive; lower seqs excluded");
    }
```

And add this test helper inside the same test module:

```rust
    async fn append_one(
        store: &InMemoryStore,
        id: &str,
        version: u64,
        expected: Option<u64>,
        payload: &[u8],
    ) {
        let env = pending_envelope(Version::new(version).unwrap())
            .event_type("E")
            .payload(payload.to_vec())
            .unwrap()
            .build();
        store
            .append(&tid(id), expected.and_then(Version::new), &[env])
            .await
            .unwrap();
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store read_all_ 2>&1 | tail -20`
Expected: FAIL to compile — `no method named read_all` / missing `AllStream`.

- [ ] **Step 3: Add the global index field**

In `crates/nexus-store/src/testing.rs`, add `use std::collections::BTreeMap;` at the top (with the other `std::collections` imports), then add the field to `InMemoryStore` (after `streams`, around line 97):

```rust
    /// All events keyed by `global_seq`, the `$all` read order. Holds the
    /// same `StoredFrame`s as `streams` (Arc-shared `Bytes`, cheap clones);
    /// written under `streams`'s lock in `append` so the two never diverge.
    global_index: Arc<Mutex<BTreeMap<u64, StoredFrame>>>,
```

And initialize it in `with_batch_size` (around line 112):

```rust
            global_index: Arc::new(Mutex::new(BTreeMap::new())),
```

- [ ] **Step 4: Write the global index in `append`**

In `InMemoryStore::append`, replace the global-seq assignment + store block (lines ~331-346, from `// Assign a store-global sequence` through `stream.extend(rows);`) with a version that also feeds the global index. Keep the lock held (atomicity, rule 1): lock order is `streams` (already held via `guard`) → `next_global_seq` → `global_index`.

```rust
        // Assign a store-global sequence to each event — monotonic across
        // all streams; gaps are permitted by the `RawEventStore` contract.
        let mut counter = self.next_global_seq.lock().await;
        let mut seq = *counter;
        let mut rows: Vec<(u64, StoredFrame)> = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            rows.push((seq.as_u64(), encode_pending_to_frame(env, seq)?));
            seq = seq
                .next()
                .ok_or(AppendError::Store(InMemoryStoreError::GlobalSeqOverflow))?;
        }
        *counter = seq;
        drop(counter);

        // Index by global_seq for $all reads, in the same critical section as
        // the per-stream store, so a reader never sees one without the other.
        {
            let mut gidx = self.global_index.lock().await;
            for (s, frame) in &rows {
                gidx.insert(*s, frame.clone());
            }
        }

        // Store the events per-stream.
        stream.extend(rows.into_iter().map(|(_, frame)| frame));
```

- [ ] **Step 5: Add `GlobalReadState` and the `read_all` impl**

Add a keyset-paginating read state near `ReadState` (after line 167) in `crates/nexus-store/src/testing.rs`:

```rust
/// Keyset-paginating read state for a one-shot `read_all` (GlobalSeq order).
struct GlobalReadState {
    global_index: Arc<Mutex<BTreeMap<u64, StoredFrame>>>,
    /// Next global_seq to scan from (inclusive). Resumes at `last + 1`.
    from: u64,
    batch_size: usize,
    buffer: VecDeque<StoredFrame>,
    done: bool,
}

impl GlobalReadState {
    async fn refill(&mut self) {
        let batch: VecDeque<StoredFrame> = {
            let guard = self.global_index.lock().await;
            guard
                .range(self.from..)
                .take(self.batch_size)
                .map(|(_, frame)| frame.clone())
                .collect()
        };
        self.done = batch.len() < self.batch_size;
        self.buffer = batch;
    }
}
```

Then add the two trait items inside `impl RawEventStore for InMemoryStore` (after `read_stream`, around line 422). The `AllStream` reuses the boxed `InMemoryStream` type:

```rust
    type AllStream = InMemoryStream;

    async fn read_all(&self, from: GlobalSeq) -> Result<Self::AllStream, Self::Error> {
        let state = GlobalReadState {
            global_index: Arc::clone(&self.global_index),
            from: from.as_u64(),
            batch_size: self.batch_size.get(),
            buffer: VecDeque::new(),
            done: false,
        };

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some(frame) = s.buffer.pop_front() {
                    return match frame_to_envelope(&frame) {
                        Ok(env) => {
                            // Resume strictly after this global_seq.
                            match env.global_seq().as_u64().checked_add(1) {
                                Some(next) => s.from = next,
                                None => s.done = true,
                            }
                            Some((Ok(env), s))
                        }
                        Err(e) => {
                            s.done = true;
                            Some((Err(e), s))
                        }
                    };
                }
                if s.done {
                    return None;
                }
                s.refill().await;
                if s.buffer.is_empty() {
                    return None;
                }
            }
        });

        Ok(InMemoryStream {
            inner: Box::pin(futures::StreamExt::fuse(unfolded)),
        })
    }
```

Note: `type AllStream` must be declared with the other associated type. Move `type AllStream = InMemoryStream;` to sit beside `type Stream = InMemoryStream;` at the top of the impl block (line ~284) if the compiler requires associated types before methods; either position compiles, keep them together for readability.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-store read_all_ 2>&1 | tail -20`
Expected: PASS — both `read_all_yields_global_order_across_streams` and `read_all_from_is_inclusive_and_resumes`.

- [ ] **Step 7: Run clippy + fmt**

Run: `nix develop -c cargo clippy -p nexus-store --all-targets 2>&1 | tail -20` then `nix develop -c cargo fmt --all`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): InMemoryStore global index + read_all (#214)"
```

---

## Task 3: fjall global-key encoding

**Files:**
- Modify: `crates/nexus-fjall/src/encoding.rs`
- Test: `crates/nexus-fjall/src/encoding.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/nexus-fjall/src/encoding.rs`:

```rust
    #[test]
    fn global_key_roundtrips() {
        let key = encode_global_key(42, 7);
        let (gseq, version) = decode_global_key(&key).unwrap();
        assert_eq!(gseq, 42);
        assert_eq!(version, 7);
    }

    #[test]
    fn global_keys_sort_by_global_seq_then_version() {
        // Big-endian → lexicographic byte order equals numeric order.
        let a = encode_global_key(1, 999);
        let b = encode_global_key(2, 1);
        assert!(a < b, "global_seq 1 must sort before global_seq 2");
    }

    #[test]
    fn decode_global_key_rejects_wrong_length() {
        assert!(decode_global_key(&[0u8; 8]).is_err());
        assert!(decode_global_key(&[0u8; 17]).is_err());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop -c cargo test -p nexus-fjall global_key 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find function encode_global_key`.

- [ ] **Step 3: Implement the encoding**

Add to `crates/nexus-fjall/src/encoding.rs` (near the event-key helpers, ~line 73). The key is `[u64 BE global_seq][u64 BE version]` — big-endian so byte order matches numeric order; version is carried because the wire-frame value does not hold it.

```rust
/// Size of a `$all` index key: `[u64 BE global_seq][u64 BE version]`.
pub const GLOBAL_KEY_SIZE: usize = 16;

/// Encode an `events_global` key as `[u64 BE global_seq][u64 BE version]`.
///
/// `global_seq` alone is unique per event; `version` is carried so the
/// read path can reconstruct a `PersistedEnvelope` (the wire-frame value
/// does not store version — fjall keeps it in the primary event key).
#[must_use]
pub fn encode_global_key(global_seq: u64, version: u64) -> [u8; GLOBAL_KEY_SIZE] {
    let mut buf = [0u8; GLOBAL_KEY_SIZE];
    buf[0..8].copy_from_slice(&global_seq.to_be_bytes());
    buf[8..16].copy_from_slice(&version.to_be_bytes());
    buf
}

/// Decode an `events_global` key into `(global_seq, version)`.
///
/// # Errors
///
/// [`DecodeError::TruncatedKey`] if `key` is not exactly
/// [`GLOBAL_KEY_SIZE`] bytes.
pub fn decode_global_key(key: &[u8]) -> Result<(u64, u64), DecodeError> {
    let bytes: [u8; GLOBAL_KEY_SIZE] = key.try_into().map_err(|_| DecodeError::TruncatedKey)?;
    let global_seq = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let version = u64::from_be_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    Ok((global_seq, version))
}
```

If `DecodeError` has no `TruncatedKey` variant, check the existing variants in `error.rs`/`encoding.rs` and reuse the one used by `decode_event_key` for a too-short key (match its style); do not invent a redundant variant. Verify with `grep -n "enum DecodeError" -A12 crates/nexus-fjall/src/encoding.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop -c cargo test -p nexus-fjall global_key 2>&1 | tail -20`
Expected: PASS — all three tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/encoding.rs
git commit -m "feat(fjall): events_global key encoding [global_seq][version] (#214)"
```

---

## Task 4: fjall `events_global` partition + append write

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` (struct field + `append`)
- Modify: `crates/nexus-fjall/src/builder.rs` (open partition)
- Test: `crates/nexus-fjall/src/store.rs` (inline tests — defer the read assertion to Task 5)

- [ ] **Step 1: Add the struct field**

In `crates/nexus-fjall/src/store.rs`, add to `pub struct FjallStore` after `events` (line 36):

```rust
    /// `$all` index: every event's wire frame keyed by
    /// `[u64 BE global_seq][u64 BE version]`. Same scan-optimized config as
    /// `events`; written in the same transaction as the primary row.
    pub(crate) events_global: fjall::SingleWriterTxKeyspace,
```

- [ ] **Step 2: Open the partition in the builder**

In `crates/nexus-fjall/src/builder.rs::open`, after the `events` partition is opened (line 108), add:

```rust
        let events_global = db.keyspace("events_global", scan_defaults)?;
```

and add `events_global,` to the `FjallStore { ... }` constructor (after `events,`, line 118).

- [ ] **Step 3: Import the global-key encoder**

In `crates/nexus-fjall/src/store.rs`, add `encode_global_key` to the `crate::encoding` import (line 2):

```rust
use crate::encoding::{decode_stream_version, encode_event_key, encode_global_key, encode_stream_version};
```

- [ ] **Step 4: Write the index row in `append`**

In `FjallStore::append`, the per-envelope loop (lines 157-186) currently ends with
`tx.insert(&self.events, &key, Slice::from(frame.value));`. Replace that single insert with a shared `Slice` written to both partitions:

```rust
            let slice = Slice::from(frame.value);
            tx.insert(&self.events, &key, slice.clone());
            let global_key = encode_global_key(global_seq, env.version().as_u64());
            tx.insert(&self.events_global, global_key, slice);
```

(`global_seq` and `env` are both in scope in this loop. `Slice` is `bytes::Bytes`-backed under `bytes_1`, so `slice.clone()` is an Arc bump, not a copy.)

- [ ] **Step 5: Write the failing test (index row exists with correct key)**

Add to the `#[cfg(test)] mod tests` in `crates/nexus-fjall/src/store.rs`:

```rust
    #[tokio::test]
    async fn append_writes_events_global_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let id = TestId("acct-1".to_owned());
        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("Created")
            .payload(b"x".to_vec())
            .unwrap()
            .build();
        store.append(&id, None, &[env]).await.unwrap();

        // The index holds exactly one row, keyed by [global_seq=1][version=1].
        let key = crate::encoding::encode_global_key(1, 1);
        let got = store.events_global.inner().get(key).unwrap();
        assert!(got.is_some(), "append must write an events_global index row");
        assert_eq!(store.events_global.inner().iter().count(), 1);
    }
```

(If `events_global.inner().get(...)` is not the right accessor, mirror however the existing tests read a partition — check for `.inner().get` / `Readable` usage already in this file.)

- [ ] **Step 6: Run the test to verify it passes**

Run: `nix develop -c cargo test -p nexus-fjall append_writes_events_global_index 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Run clippy + fmt**

Run: `nix develop -c cargo clippy -p nexus-fjall --all-targets 2>&1 | tail -20` then `nix develop -c cargo fmt --all`
Expected: no warnings. (`FjallStore` still misses `AllStream`/`read_all` — that is Task 5; build of the *lib* will fail until then, so run the targeted test which compiles the test cfg. If the crate won't build at all because the trait is unsatisfied, do Task 5 before running the gate.)

- [ ] **Step 8: Commit**

```bash
git add crates/nexus-fjall/src/store.rs crates/nexus-fjall/src/builder.rs
git commit -m "feat(fjall): events_global partition written in append tx (#214)"
```

---

## Task 5: `FjallAllStream` + fjall `read_all`

**Files:**
- Create: `crates/nexus-fjall/src/all_stream.rs`
- Modify: `crates/nexus-fjall/src/lib.rs` (`mod all_stream;`)
- Modify: `crates/nexus-fjall/src/store.rs` (`read_all` impl + `AllStream`)
- Test: `crates/nexus-fjall/src/all_stream.rs` (inline) + `crates/nexus-fjall/src/store.rs`

- [ ] **Step 1: Create the cursor with a failing decode test**

Create `crates/nexus-fjall/src/all_stream.rs`. This mirrors `FjallStream` (`stream.rs`) but scans `events_global` and decodes the version from the key. Start with the decode function and its test:

```rust
use std::collections::VecDeque;

use bytes::Bytes;
use fjall::{SingleWriterTxKeyspace, Slice};
use nexus::Version;
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;

use crate::encoding::{decode_event_value, decode_global_key, encode_global_key};
use crate::error::FjallError;

/// Decode one `events_global` `(key, value)` row into a [`PersistedEnvelope`].
///
/// The `global_seq` in the key and in the frame value are validated to match
/// (CLAUDE.md rule 4 — redundant data must be validated). The value is the
/// identical wire frame stored in the primary `events` partition, so it flows
/// zero-copy into the envelope.
fn decode_global_row(key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError> {
    let (key_global_seq, version_raw) =
        decode_global_key(key).map_err(|_| FjallError::CorruptValue {
            stream_id: arrayvec::ArrayString::new(),
            version: None,
        })?;

    let bytes_value: Bytes = value.into();
    let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
        stream_id: arrayvec::ArrayString::new(),
        version: Some(version_raw),
    })?;

    if decoded.global_seq != key_global_seq {
        return Err(FjallError::CorruptValue {
            stream_id: arrayvec::ArrayString::new(),
            version: Some(version_raw),
        });
    }

    let version = Version::new(version_raw).ok_or(FjallError::CorruptValue {
        stream_id: arrayvec::ArrayString::new(),
        version: Some(version_raw),
    })?;
    let global_seq = GlobalSeq::new(decoded.global_seq).ok_or(FjallError::CorruptValue {
        stream_id: arrayvec::ArrayString::new(),
        version: Some(version_raw),
    })?;

    PersistedEnvelope::try_new(
        version,
        global_seq,
        bytes_value,
        decoded.schema_version,
        decoded.event_type_range,
        decoded.payload_range,
        decoded.metadata_range,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id: arrayvec::ArrayString::new(),
        version: version_raw,
        source,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use crate::encoding::encode_event_value;

    fn global_row(global_seq: u64, version: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_global_key(global_seq, version);
        let mut val = Vec::new();
        encode_event_value(&mut val, global_seq, 1, et, None, payload).unwrap();
        (Slice::from(&key[..]), Slice::from(val))
    }

    #[test]
    fn decode_global_row_yields_envelope() {
        let (k, v) = global_row(42, 7, "Created", b"data");
        let env = decode_global_row(&k, v).unwrap();
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn decode_global_row_rejects_global_seq_mismatch() {
        // Key says global_seq 99, value frame says 42 → corruption.
        let key = encode_global_key(99, 7);
        let mut val = Vec::new();
        encode_event_value(&mut val, 42, 1, "E", None, b"x").unwrap();
        let err = decode_global_row(&Slice::from(&key[..]), Slice::from(val)).unwrap_err();
        assert!(matches!(err, FjallError::CorruptValue { .. }));
    }
}
```

Verify the `FjallError::CorruptValue`/`EnvelopeCorrupt` field names by checking `crates/nexus-fjall/src/error.rs`; the `$all` index has no single stream id, so an empty `ArrayString` is the honest label. If `FjallError` requires a non-empty diagnostic, add a literal like `ArrayString::from("$all").unwrap()` instead — confirm the field type first.

- [ ] **Step 2: Register the module and run the decode test**

In `crates/nexus-fjall/src/lib.rs` add `mod all_stream;` next to `mod stream;`.

Run: `nix develop -c cargo test -p nexus-fjall decode_global_row 2>&1 | tail -20`
Expected: PASS — both decode tests.

- [ ] **Step 3: Add the `FjallAllStream` cursor body**

Append to `crates/nexus-fjall/src/all_stream.rs` (before the test module). This mirrors `FjallStream` but the keyset cursor is on `global_seq` and the scan range spans all stream ids:

```rust
/// `futures::Stream` of fjall events in `GlobalSeq` order (the `$all` read).
///
/// Created by `FjallStore::read_all`. Loads at most `batch_size` rows per
/// batch via keyset pagination on `global_seq`, so memory is bounded
/// regardless of how many events exist.
pub struct FjallAllStream {
    events: VecDeque<(Slice, Slice)>,
    keyspace: SingleWriterTxKeyspace,
    /// Next global_seq to scan from (inclusive).
    next_global_seq: u64,
    batch_size: usize,
    done: bool,
    poisoned: bool,
}

impl FjallAllStream {
    /// Build and eagerly load the first batch, scanning from `from` (inclusive).
    pub(crate) fn new(
        keyspace: SingleWriterTxKeyspace,
        from: u64,
        batch_size: usize,
    ) -> Result<Self, FjallError> {
        let mut stream = Self {
            events: VecDeque::new(),
            keyspace,
            next_global_seq: from,
            batch_size,
            done: false,
            poisoned: false,
        };
        stream.refill()?;
        Ok(stream)
    }

    fn refill(&mut self) -> Result<(), FjallError> {
        // [global_seq=from][version=0] .. [global_seq=MAX][version=MAX].
        // global_seq is the high-order 8 bytes, so this spans every event with
        // global_seq >= next_global_seq across all streams.
        let start = encode_global_key(self.next_global_seq, 0);
        let end = encode_global_key(u64::MAX, u64::MAX);

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

    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        loop {
            if let Some((key, value)) = self.events.pop_front() {
                return Some(match decode_global_row(&key, value) {
                    Ok(env) => {
                        // Resume strictly after this global_seq.
                        match env.global_seq().as_u64().checked_add(1) {
                            Some(n) => self.next_global_seq = n,
                            None => self.done = true,
                        }
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
                return None;
            }
        }
    }
}

impl futures::Stream for FjallAllStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.poll_one())
    }
}
```

- [ ] **Step 4: Implement `read_all` on `FjallStore`**

In `crates/nexus-fjall/src/store.rs`, add the import `use crate::all_stream::FjallAllStream;` (top, with the other `crate::` imports) and add the two trait items inside `impl RawEventStore for FjallStore` — `type AllStream` beside `type Stream` (line ~58), and `read_all` after `read_stream`:

```rust
    type AllStream = FjallAllStream;
```

```rust
    async fn read_all(&self, from: GlobalSeq) -> Result<Self::AllStream, Self::Error> {
        FjallAllStream::new(self.events_global.clone(), from.as_u64(), self.batch_size.get())
    }
```

Add `use nexus_store::GlobalSeq;` to the imports if not already present.

- [ ] **Step 5: Write the end-to-end read test**

Add to `crates/nexus-fjall/src/store.rs` tests:

```rust
    #[tokio::test]
    async fn read_all_yields_global_order_across_streams() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let a = TestId("a".to_owned());
        let b = TestId("b".to_owned());

        let mk = |v: u64, p: &[u8]| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("E")
                .payload(p.to_vec())
                .unwrap()
                .build()
        };
        store.append(&a, None, &[mk(1, b"a1")]).await.unwrap();
        store.append(&b, None, &[mk(1, b"b1")]).await.unwrap();
        store.append(&a, Some(Version::new(1).unwrap()), &[mk(2, b"a2")]).await.unwrap();

        let mut all = store.read_all(GlobalSeq::INITIAL).await.unwrap();
        let mut seen = Vec::new();
        while let Some(env) = all.next().await {
            let env = env.unwrap();
            seen.push((env.global_seq().as_u64(), env.payload().to_vec()));
        }
        assert_eq!(
            seen,
            vec![(1, b"a1".to_vec()), (2, b"b1".to_vec()), (3, b"a2".to_vec())],
        );
    }

    #[tokio::test]
    async fn read_all_from_is_inclusive() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let a = TestId("a".to_owned());
        let mk = |v: u64| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("E").payload(vec![v as u8]).unwrap().build()
        };
        store.append(&a, None, &[mk(1)]).await.unwrap();
        store.append(&a, Some(Version::new(1).unwrap()), &[mk(2)]).await.unwrap();
        store.append(&a, Some(Version::new(2).unwrap()), &[mk(3)]).await.unwrap();

        let mut all = store.read_all(GlobalSeq::new(2).unwrap()).await.unwrap();
        let mut seqs = Vec::new();
        while let Some(env) = all.next().await {
            seqs.push(env.unwrap().global_seq().as_u64());
        }
        assert_eq!(seqs, vec![2, 3]);
    }
```

- [ ] **Step 6: Run the tests + full crate build**

Run: `nix develop -c cargo test -p nexus-fjall read_all 2>&1 | tail -25`
Expected: PASS. Then `nix develop -c cargo build -p nexus-fjall -p nexus-store` — Expected: clean (all trait items now implemented).

- [ ] **Step 7: Lifecycle test (write → close → reopen → read_all)**

Defensive boundary + lifecycle (CLAUDE.md testing category 2). Add to `crates/nexus-fjall/src/store.rs` tests:

```rust
    #[tokio::test]
    async fn read_all_survives_reopen() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let a = TestId("a".to_owned());
        {
            let store = FjallStore::builder(&path).open().unwrap();
            let env = pending_envelope(Version::new(1).unwrap())
                .event_type("E").payload(b"x".to_vec()).unwrap().build();
            store.append(&a, None, &[env]).await.unwrap();
        }
        let store = FjallStore::builder(&path).open().unwrap();
        let mut all = store.read_all(GlobalSeq::INITIAL).await.unwrap();
        let env = all.next().await.unwrap().unwrap();
        assert_eq!(env.global_seq().as_u64(), 1);
        assert_eq!(env.payload(), b"x");
    }
```

Run: `nix develop -c cargo test -p nexus-fjall read_all_survives_reopen 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 8: clippy + fmt, stage new file, commit**

```bash
nix develop -c cargo clippy -p nexus-fjall --all-targets 2>&1 | tail -20
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/all_stream.rs crates/nexus-fjall/src/lib.rs crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): FjallAllStream + read_all (#214)"
```

- [ ] **Step 9: Open PR 1**

```bash
git push -u origin <branch>
gh pr create --title "feat: $all read path — read_all + events_global index (#214)" \
  --body "Phase 1 of #214: GlobalSeq-ordered read_all on RawEventStore, events_global index written in the append tx, both adapters. Subscription cursor follows in PR 2."
```

Let the pre-commit hook run the gate. Address any failures, then squash-merge: `gh pr merge --squash --delete-branch`.

---

# PHASE 2 — Subscription Cursor (PR 2)

## Task 6: store-wide `wake_all` on `StreamNotifiers`

**Files:**
- Modify: `crates/nexus-store/src/notify.rs`
- Test: `crates/nexus-store/src/notify.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/nexus-store/src/notify.rs`:

```rust
    /// A waiter on the store-wide $all notifier wakes on wake_all(), and is
    /// NOT roused by a per-stream wake().
    #[tokio::test]
    async fn wake_all_rouses_all_waiter_only() {
        let reg = StreamNotifiers::new();
        let notifier = Arc::clone(reg.all_notifier());
        let start = Arc::new(Barrier::new(2));
        let start_sub = Arc::clone(&start);
        let sub = tokio::spawn(async move {
            let notified = notifier.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            start_sub.wait().await;
            notified.await;
        });
        start.wait().await;

        // A per-stream wake must NOT rouse the $all waiter.
        reg.wake(b"some-stream");
        assert!(
            timeout(MUST_NOT_WAKE, &mut { &mut sub }).await.is_err()
                || !sub.is_finished(),
            "per-stream wake must not wake the $all waiter"
        );

        reg.wake_all();
        timeout(MUST_WAKE, sub)
            .await
            .expect("wake_all must rouse the $all waiter")
            .unwrap();
    }
```

If the `&mut { &mut sub }` borrow dance for the negative check is awkward, simplify by structuring it like the existing `wake_is_isolated_per_stream` test (which already does the "must NOT wake then must wake" pattern with a `&mut sub`). Mirror that test's exact shape.

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store wake_all 2>&1 | tail -20`
Expected: FAIL to compile — `no method named all_notifier` / `wake_all`.

- [ ] **Step 3: Add the field + methods, hand-write `Default`**

In `crates/nexus-store/src/notify.rs`:

Change the derive on `StreamNotifiers` from `#[derive(Debug, Default)]` to `#[derive(Debug)]` (a field of `Arc<Notify>` is not `Default`), and add the field:

```rust
#[derive(Debug)]
pub struct StreamNotifiers {
    map: Mutex<HashMap<Box<[u8]>, Entry, RandomState>>,
    /// Store-wide notifier for `$all` subscribers. Always present (no
    /// drop-guard / refcount): every commit wakes it, and every `$all`
    /// subscriber genuinely wants every event, so there is no thundering
    /// herd to avoid here — unlike the per-stream `map`.
    all: Arc<Notify>,
}

impl Default for StreamNotifiers {
    fn default() -> Self {
        Self {
            map: Mutex::default(),
            all: Arc::new(Notify::new()),
        }
    }
}
```

Add to `impl StreamNotifiers` (near `wake`):

```rust
    /// Wake every task parked on the store-wide `$all` notifier.
    ///
    /// MUST be called *after* the corresponding event(s) are durably
    /// committed, so a woken `$all` subscriber re-reads visible data. A no-op
    /// when no `$all` subscriber is parked.
    pub fn wake_all(&self) {
        self.all.notify_waiters();
    }

    /// The store-wide `$all` notifier. An all-streams subscription cursor
    /// clones this and parks on it; obey the same enable-before-read ordering
    /// as the per-stream path (see the module-level ordering contract).
    #[must_use]
    pub const fn all_notifier(&self) -> &Arc<Notify> {
        &self.all
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `nix develop -c cargo test -p nexus-store wake_all 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/notify.rs
git commit -m "feat(store): store-wide wake_all on StreamNotifiers (#214)"
```

---

## Task 7: `RawAllSubscription` trait + `Subscription::subscribe_all`

**Files:**
- Modify: `crates/nexus-store/src/subscription.rs`

- [ ] **Step 1: Add the trait**

In `crates/nexus-store/src/subscription.rs`, add `use nexus_store...`? No — it is in `nexus-store`. Add `use nexus_store::store::GlobalSeq;`? It is the same crate: add `use crate::store::GlobalSeq;` to the imports (alongside the existing `use nexus::{Id, Version};`). Then add the trait after `RawSubscription` (after line 82):

```rust
/// Adapter-facing primitive for all-streams (`$all`) subscriptions.
///
/// The dual of [`RawSubscription`] for the store-wide [`GlobalSeq`] order:
/// no stream id, resumes on `GlobalSeq` instead of `Version`. The
/// user-facing [`Subscription`] struct composes this into `subscribe_all`.
///
/// # Contract
///
/// - `from: None` → start from the first event ever appended.
/// - `from: Some(g)` → start from the event *after* `GlobalSeq` `g`.
/// - The returned stream **never returns `None`** — it waits for new events
///   when caught up rather than terminating.
/// - Events are yielded in ascending `GlobalSeq` order; the sequence is
///   monotonic but not gapless, and the cursor tolerates gaps.
pub trait RawAllSubscription: sealed::Sealed + Send + Sync + 'static {
    /// The cursor type — a `futures::Stream` of envelopes, `'static`.
    type Stream: EventStream<Error = Self::Error> + 'static;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Open an all-streams subscription cursor.
    fn subscribe_all(
        arc: &Arc<Self>,
        from: Option<GlobalSeq>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}
```

- [ ] **Step 2: Relax `Subscription::new` and add `subscribe_all`**

The `new` constructor needs no trait bound (it only clones the `Arc`). Move it out of the `impl<S: RawSubscription>` block into an unconstrained block so a store implementing only `RawAllSubscription` can still construct a `Subscription`. Replace the `impl<S: RawSubscription> Subscription<S>` block (lines 110-142) with:

```rust
impl<S> Subscription<S> {
    /// Construct from a [`Store<S>`] handle. One `Arc::clone` per call.
    #[must_use]
    pub fn new(store: &Store<S>) -> Self {
        Self {
            store: Arc::clone(store.arc()),
        }
    }
}

impl<S: RawSubscription> Subscription<S> {
    /// Open a per-stream subscription cursor.
    ///
    /// `from: None` starts from version 1; `from: Some(v)` starts from the
    /// event *after* version `v`. The returned cursor **never returns
    /// `None`** — it waits for new events when caught up.
    ///
    /// # Errors
    ///
    /// Returns `S::Error` if the adapter cannot open the cursor.
    pub async fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<<S as RawSubscription>::Stream, <S as RawSubscription>::Error> {
        S::subscribe(&self.store, id, from).await
    }
}

impl<S: RawAllSubscription> Subscription<S> {
    /// Open an all-streams (`$all`) subscription cursor, ordered by
    /// [`GlobalSeq`].
    ///
    /// `from: None` starts from the first event ever appended; `from:
    /// Some(g)` starts from the event *after* `GlobalSeq` `g`. The returned
    /// cursor **never returns `None`** — it waits for new events when caught
    /// up.
    ///
    /// # Errors
    ///
    /// Returns `S::Error` if the adapter cannot open the cursor (e.g. an
    /// arithmetic overflow if `from` is `Some(GlobalSeq::MAX)`).
    pub async fn subscribe_all(
        &self,
        from: Option<GlobalSeq>,
    ) -> Result<<S as RawAllSubscription>::Stream, <S as RawAllSubscription>::Error> {
        S::subscribe_all(&self.store, from).await
    }
}
```

(The fully-qualified `<S as RawSubscription>::Stream` is required because a store implementing both traits has two associated `Stream` types.)

- [ ] **Step 2b: Verify it fails (adapters miss `RawAllSubscription`)**

Run: `nix develop -c cargo build -p nexus-store`
Expected: PASS for the trait/method themselves (no impls referenced yet). The adapters gain their impls in Tasks 8 and 9.

- [ ] **Step 3: Commit**

```bash
nix develop -c cargo fmt --all
git add crates/nexus-store/src/subscription.rs
git commit -m "feat(store): RawAllSubscription trait + Subscription::subscribe_all (#214)"
```

---

## Task 8: `InMemoryStore` `$all` subscription

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`
- Test: `crates/nexus-store/src/testing.rs` (inline tests)

- [ ] **Step 1: Wake `$all` in `append`**

In `InMemoryStore::append`, the notify block (lines ~356-358) currently calls only `self.notifiers.wake(id.as_ref())`. Add the store-wide wake:

```rust
        if should_notify {
            self.notifiers.wake(id.as_ref());
            self.notifiers.wake_all();
        }
```

- [ ] **Step 2: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/nexus-store/src/testing.rs`:

```rust
    #[tokio::test]
    async fn subscribe_all_catches_up_then_sees_live_event() {
        use crate::subscription::RawAllSubscription;
        use futures::StreamExt;

        let store = Arc::new(InMemoryStore::new());
        append_one(&store, "a", 1, None, b"a1").await;
        append_one(&store, "b", 1, None, b"b1").await;

        let mut sub = InMemoryStore::subscribe_all(&store, None).await.unwrap();
        // Catch-up: the two pre-existing events in GlobalSeq order.
        assert_eq!(sub.next().await.unwrap().unwrap().global_seq().as_u64(), 1);
        assert_eq!(sub.next().await.unwrap().unwrap().global_seq().as_u64(), 2);

        // Live: a later append on ANY stream wakes the $all cursor.
        let store2 = Arc::clone(&store);
        tokio::spawn(async move {
            append_one(&store2, "a", 2, Some(1), b"a2").await;
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.global_seq().as_u64(), 3);
        assert_eq!(live.payload(), b"a2");
    }
```

(`append_one` is the helper added in Task 2. If Phase 2 is a fresh branch off `main` after PR 1 merged, the helper is already present.)

- [ ] **Step 3: Run the test to verify it fails**

Run: `nix develop -c cargo test -p nexus-store subscribe_all_catches_up 2>&1 | tail -20`
Expected: FAIL to compile — `RawAllSubscription not implemented for InMemoryStore`.

- [ ] **Step 4: Implement the cursor + trait**

Add to `crates/nexus-store/src/testing.rs`. The cursor mirrors the per-stream `SubState`/`InMemorySubscriptionStream`, but resumes on `global_seq`, scans `global_index`, parks on the store-wide notifier (no `SubscriptionGuard` — the `$all` notifier is always present), and **never** returns `None`.

```rust
/// Inner state for the `$all` subscription generator. Owns
/// `Arc<InMemoryStore>`, so `'static`. Never terminates: waits on the
/// store-wide notifier when caught up.
struct AllSubState {
    store: Arc<InMemoryStore>,
    buffer: VecDeque<StoredFrame>,
    /// Next global_seq to scan from (inclusive).
    next_global_seq: u64,
    batch_size: usize,
}

impl AllSubState {
    async fn refill(&mut self) {
        let buffer: VecDeque<StoredFrame> = {
            let guard = self.store.global_index.lock().await;
            guard
                .range(self.next_global_seq..)
                .take(self.batch_size)
                .map(|(_, frame)| frame.clone())
                .collect()
        };
        self.buffer = buffer;
    }
}

/// `futures::Stream` for an in-memory `$all` subscription. Never returns
/// `None` — blocks on the store-wide notifier until new events arrive.
pub struct InMemoryAllSubscriptionStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, InMemoryStoreError>> + Send>,
    >,
}

impl futures::Stream for InMemoryAllSubscriptionStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl RawAllSubscription for InMemoryStore {
    type Stream = InMemoryAllSubscriptionStream;
    type Error = InMemoryStoreError;

    async fn subscribe_all(
        arc: &Arc<Self>,
        from: Option<GlobalSeq>,
    ) -> Result<InMemoryAllSubscriptionStream, InMemoryStoreError> {
        let start = match from {
            None => GlobalSeq::INITIAL,
            Some(g) => g.next().ok_or(InMemoryStoreError::GlobalSeqOverflow)?,
        };
        let state = AllSubState {
            store: Arc::clone(arc),
            buffer: VecDeque::new(),
            next_global_seq: start.as_u64(),
            batch_size: arc.batch_size().get(),
        };

        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some(frame) = s.buffer.pop_front() {
                    return match frame_to_envelope(&frame) {
                        Ok(env) => {
                            match env.global_seq().as_u64().checked_add(1) {
                                Some(n) => s.next_global_seq = n,
                                None => {
                                    return Some((
                                        Err(InMemoryStoreError::GlobalSeqOverflow),
                                        s,
                                    ));
                                }
                            }
                            Some((Ok(env), s))
                        }
                        Err(e) => Some((Err(e), s)),
                    };
                }

                // Buffer empty. Register the wait BEFORE refilling to close the
                // lost-wakeup race (same discipline as the per-stream cursor).
                let notify = Arc::clone(s.store.notifiers.all_notifier());
                let notified = notify.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable();

                s.refill().await;
                if s.buffer.is_empty() {
                    notified.await;
                }
            }
        });

        Ok(InMemoryAllSubscriptionStream {
            inner: Box::pin(unfolded),
        })
    }
}
```

Note: `RawAllSubscription` and `GlobalSeq` must be imported at the top of `testing.rs`: add `RawAllSubscription` to the `use crate::subscription::{...}` line, and `GlobalSeq` is already imported via `use crate::store::{GlobalSeq, RawEventStore};`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `nix develop -c cargo test -p nexus-store subscribe_all_catches_up 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Add a concurrency test (CLAUDE.md testing category 4)**

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn subscribe_all_sees_concurrent_appends_across_streams() {
        use crate::subscription::RawAllSubscription;
        use futures::StreamExt;

        let store = Arc::new(InMemoryStore::new());
        let mut sub = InMemoryStore::subscribe_all(&store, None).await.unwrap();

        // Two writers append to two different streams concurrently.
        let s1 = Arc::clone(&store);
        let s2 = Arc::clone(&store);
        let w1 = tokio::spawn(async move {
            for v in 1..=10 {
                append_one(&s1, "x", v, (v > 1).then(|| v - 1), b"x").await;
            }
        });
        let w2 = tokio::spawn(async move {
            for v in 1..=10 {
                append_one(&s2, "y", v, (v > 1).then(|| v - 1), b"y").await;
            }
        });
        w1.await.unwrap();
        w2.await.unwrap();

        // The cursor observes 20 events in strictly ascending GlobalSeq order.
        let mut prev = 0u64;
        for _ in 0..20 {
            let g = sub.next().await.unwrap().unwrap().global_seq().as_u64();
            assert!(g > prev, "global_seq must be strictly increasing: {g} after {prev}");
            prev = g;
        }
    }
```

Run: `nix develop -c cargo test -p nexus-store subscribe_all_sees_concurrent 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: clippy + fmt + commit**

```bash
nix develop -c cargo clippy -p nexus-store --all-targets 2>&1 | tail -20
nix develop -c cargo fmt --all
git add crates/nexus-store/src/testing.rs
git commit -m "feat(store): InMemoryStore $all subscription cursor (#214)"
```

---

## Task 9: `FjallStore` `$all` subscription

**Files:**
- Create: `crates/nexus-fjall/src/all_subscription_stream.rs`
- Modify: `crates/nexus-fjall/src/lib.rs` (`mod all_subscription_stream;`)
- Modify: `crates/nexus-fjall/src/store.rs` (wake_all in append + `RawAllSubscription` impl)
- Test: `crates/nexus-fjall/src/all_subscription_stream.rs` (inline)

- [ ] **Step 1: Wake `$all` in `append`**

In `FjallStore::append`, after the existing `self.notifiers.wake(id_bytes);` (line 209), add:

```rust
        self.notifiers.wake_all();
```

- [ ] **Step 2: Create the never-ending cursor**

Create `crates/nexus-fjall/src/all_subscription_stream.rs`, mirroring `subscription_stream.rs` (`FjallSubscriptionStream`) but driving `FjallAllStream` and parking on the store-wide notifier (no per-stream guard, no `OwnedStreamId`):

```rust
use std::sync::Arc;

use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;
use nexus_store::store::RawEventStore;

use crate::all_stream::FjallAllStream;
use crate::error::FjallError;
use crate::store::FjallStore;

/// `futures::Stream` `$all` subscription owning an `Arc<FjallStore>`.
///
/// `'static`, spawnable. Holds an eagerly-loaded [`FjallAllStream`] batch;
/// when drained, waits on the store-wide notifier and re-reads from the last
/// yielded `GlobalSeq`. **Never returns `None`**.
pub struct FjallAllSubscriptionStream {
    inner: core::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PersistedEnvelope, FjallError>> + Send>,
    >,
}

struct AllSubState {
    store: Arc<FjallStore>,
    inner: FjallAllStream,
    /// Next global_seq to read from (inclusive) on refill.
    next_global_seq: u64,
}

impl AllSubState {
    async fn refill(&mut self) -> Result<(), FjallError> {
        let from = GlobalSeq::new(self.next_global_seq).unwrap_or(GlobalSeq::INITIAL);
        self.inner = self.store.read_all(from).await?;
        Ok(())
    }
}

impl FjallAllSubscriptionStream {
    pub(crate) fn new(
        store: Arc<FjallStore>,
        inner: FjallAllStream,
        from: GlobalSeq,
    ) -> Self {
        let state = AllSubState {
            store,
            inner,
            next_global_seq: from.as_u64(),
        };
        let unfolded = futures::stream::unfold(state, |mut s| async move {
            loop {
                if let Some(item) = s.inner.poll_one() {
                    match item {
                        Ok(env) => {
                            match env.global_seq().as_u64().checked_add(1) {
                                Some(n) => s.next_global_seq = n,
                                None => return Some((Err(FjallError::GlobalSeqOverflow), s)),
                            }
                            return Some((Ok(env), s));
                        }
                        Err(e) => return Some((Err(e), s)),
                    }
                }

                // Buffer empty. Register the wait BEFORE refilling (lost-wakeup
                // race), parking on the store-wide $all notifier.
                let notify = Arc::clone(s.store.notifiers.all_notifier());
                let notified = notify.notified();
                tokio::pin!(notified);
                let _ = notified.as_mut().enable();

                if let Err(e) = s.refill().await {
                    return Some((Err(e), s));
                }
                if s.inner.is_empty() {
                    notified.await;
                }
            }
        });
        Self {
            inner: Box::pin(unfolded),
        }
    }
}

impl futures::Stream for FjallAllSubscriptionStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
```

Two things to verify against the real code: (a) `FjallStore::notifiers` must be reachable from this module — it is `pub(crate)`, so OK. (b) `FjallError::GlobalSeqOverflow` exists (it does, per `error.rs`). The `unwrap_or(GlobalSeq::INITIAL)` only triggers if `next_global_seq` is 0, which never happens after the first yield (global_seq ≥ 1); the very first refill uses the constructor's `from`.

- [ ] **Step 3: Register module + add `RawAllSubscription` impl**

In `crates/nexus-fjall/src/lib.rs` add `mod all_subscription_stream;`.

In `crates/nexus-fjall/src/store.rs`, add imports `use crate::all_subscription_stream::FjallAllSubscriptionStream;` and `use nexus_store::subscription::RawAllSubscription;` (extend the existing `subscription::{...}` import). Then add after the `RawSubscription` impl (after line 321):

```rust
impl RawAllSubscription for FjallStore {
    type Stream = FjallAllSubscriptionStream;
    type Error = FjallError;

    async fn subscribe_all(
        arc: &Arc<Self>,
        from: Option<GlobalSeq>,
    ) -> Result<FjallAllSubscriptionStream, FjallError> {
        let start = match from {
            None => GlobalSeq::INITIAL,
            Some(g) => g.next().ok_or(FjallError::GlobalSeqOverflow)?,
        };
        let inner = arc.read_all(start).await?;
        Ok(FjallAllSubscriptionStream::new(Arc::clone(arc), inner, start))
    }
}
```

(`sealed::Sealed for FjallStore` is already implemented at line 290 — `RawAllSubscription`'s `sealed::Sealed` supertrait is satisfied. Add `use nexus_store::GlobalSeq;` if not already imported.)

- [ ] **Step 4: Write the catch-up + live test**

Add to `crates/nexus-fjall/src/all_subscription_stream.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use futures::StreamExt;
    use nexus::Version;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::subscription::RawAllSubscription;

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

    #[tokio::test]
    async fn subscribe_all_catches_up_then_sees_live() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FjallStore::builder(dir.path().join("db")).open().unwrap());
        let a = Tid("a".to_owned());
        let b = Tid("b".to_owned());
        let mk = |v: u64, p: &[u8]| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("E").payload(p.to_vec()).unwrap().build()
        };
        store.append(&a, None, &[mk(1, b"a1")]).await.unwrap();
        store.append(&b, None, &[mk(1, b"b1")]).await.unwrap();

        let mut sub = FjallStore::subscribe_all(&store, None).await.unwrap();
        assert_eq!(sub.next().await.unwrap().unwrap().global_seq().as_u64(), 1);
        assert_eq!(sub.next().await.unwrap().unwrap().global_seq().as_u64(), 2);

        let store2 = Arc::clone(&store);
        tokio::spawn(async move {
            let a2 = Tid("a".to_owned());
            store2
                .append(&a2, Some(Version::new(1).unwrap()), &[mk(2, b"a2")])
                .await
                .unwrap();
        });
        let live = sub.next().await.unwrap().unwrap();
        assert_eq!(live.global_seq().as_u64(), 3);
        assert_eq!(live.payload(), b"a2");
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `nix develop -c cargo test -p nexus-fjall subscribe_all_catches_up 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 6: clippy + fmt, stage new file, commit**

```bash
nix develop -c cargo clippy -p nexus-fjall --all-targets 2>&1 | tail -20
nix develop -c cargo fmt --all
git add crates/nexus-fjall/src/all_subscription_stream.rs crates/nexus-fjall/src/lib.rs crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): FjallStore $all subscription cursor (#214)"
```

- [ ] **Step 7: Open PR 2**

```bash
git push -u origin <branch>
gh pr create --title "feat: $all subscription cursor (#214)" \
  --body "Phase 2 of #214: never-ending GlobalSeq-ordered subscription. wake_all on StreamNotifiers, RawAllSubscription trait, Subscription::subscribe_all, both adapters. Closes #214."
```

Let the pre-commit hook run the gate; fix any failures; squash-merge `gh pr merge --squash --delete-branch`.

---

## Self-Review Checklist (completed during planning)

- **Spec coverage:** `$all` cursor over the store ordered by GlobalSeq (Tasks 5, 9) ✓; never terminates / waits when caught up (Tasks 8, 9 — `notified.await`) ✓; `from: None` = beginning, `Some(g)` = after `g` (Tasks 8, 9 — `g.next()`) ✓; yields raw `PersistedEnvelope` (all read/cursor tasks) ✓; tolerates GlobalSeq gaps (range scan `>= from`, resume `last+1` only as keyset cursor, never assumes contiguity — Tasks 2, 5) ✓; sibling of per-stream subscription (`subscribe_all` on `Subscription`, Task 7) ✓. Depends on #125 (per-stream subscription) — already merged.
- **Type consistency:** `read_all(from: GlobalSeq)` (inclusive) on the trait; `subscribe_all(from: Option<GlobalSeq>)` on the cursor — Option→inclusive translation via `g.next()` in both adapters, matching the per-stream `v.next()` convention. `AllStream` associated type distinct from `Stream`. fjall key `[u64 BE global_seq][u64 BE version]` used identically by `encode_global_key` (Task 3), the append write (Task 4), and `FjallAllStream::refill`/`decode_global_row` (Task 5).
- **Out of scope (confirmed not built):** exactly-once checkpoint / competing consumers (agency#155); trait-level filtering / ring-buffer notification (nexus Tier 3).
