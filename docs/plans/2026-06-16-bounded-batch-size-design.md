# Bounded Subscription / Read Batch Size

**Date:** 2026-06-16
**Status:** Approved
**Issue:** [#176](https://github.com/devrandom-labs/nexus/issues/176) — *Bound subscription/read batch size: compile-time const cap + runtime config*
**Scope:** `nexus-store` (`testing.rs` in-memory adapter, a new batch-size const), `nexus-fjall` (store, stream, subscription cursor, builder)

## Problem

`FjallStore::read_stream` and both subscription cursors load the **entire** remaining
stream — `from_version..=u64::MAX` — into a single `Vec` before yielding the first
event:

- `crates/nexus-fjall/src/store.rs:254` — `range(start..=end).collect::<Vec<_>>()`
- `crates/nexus-fjall/src/subscription_stream.rs:157` — `refill` calls `read_stream`
- `crates/nexus-store/src/testing.rs:315` — `read_stream` filters + collects all rows
- `crates/nexus-store/src/testing.rs:365` — in-memory subscription `refill`, same shape

There is no batch-size cap, no compile-time bound, no operator-configurable limit. A
consumer that falls behind a high-throughput writer, a large catch-up replay, or a
pathological aggregate stream loads an unbounded number of events into RAM before the
first yield. On the project's IoT/mobile targets this can OOM the process.

## Goals

1. **Hard resident-memory bound.** No read path ever holds more than `batch_size` event
   rows in memory at once.
2. **Compile-time ceiling.** A `const MAX_BATCH` baked into the binary; runtime config
   that exceeds it is rejected at builder time, not silently clamped.
3. **Operator-configurable batch size.** `FjallStoreBuilder::batch_size(n)` and
   `InMemoryStore::with_batch_size(n)`, defaulting to a sane value.
4. **Transparent refill on the one-shot read path too.** `FjallStream` / `InMemoryStream`
   paginate internally until the persisted stream is exhausted — callers see the same
   `next()` contract; chunking is invisible.
5. **No churn to the trait surface.** `RawEventStore::read_stream(&self, …)` keeps its
   signature; the change stays inside the two adapters plus one shared const.

## Non-Goals

- Backpressure on writers when subscribers are slow (separate, larger design — see issue).
- A streaming-without-iterator cursor (per-event fjall range scan) — explicitly ruled out
  in the design walkthrough as the wrong tradeoff.
- Bounding the *global* (all-streams) log scan — no such read path exists yet.

## Key facts established during design

These are load-bearing; the design depends on them.

### Batch buffer: `Vec` + `take(batch_size)`, not `ArrayVec`

Both options bound resident memory equally — neither scans more than `batch_size` rows.
`ArrayVec<(Slice, Slice), MAX>`'s only added property is stack residence (no heap alloc),
but `(Slice, Slice)` is ~64 bytes (`bytes::Bytes` is four machine words), so
`ArrayVec<_, 4096>` is ~256 KiB **inline on the stack**, moved with the cursor — the wrong
place for a buffer of that size on the small-stack targets we care about. It also has no
O(1) `pop_front`, so the existing front-drain `VecDeque` cursor would have to become an
index cursor. `Vec`/`VecDeque` capped by `.take(batch_size)` bounds memory just as hard,
puts the buffer on the heap where it belongs, validates against the `const` ceiling, and
fits the existing cursor unchanged.

### fjall `range()` is lazy — `.take(batch_size)` *is* the bound

`Keyspace::range()` (fjall 3.1.4, `keyspace/mod.rs:444`) returns `Iter`, a
`Box<dyn DoubleEndedIterator>` (`iter.rs:7`) pulled one `next()` at a time. `.take(n)`
genuinely stops the LSM scan after `n` items — it does not scan-then-truncate. fjall's own
docs warn against unbounded ranges *"unless limited"*; `.take()` is that limiting. So the
bound is literally `range(start..=end).take(batch_size)`.

### The fjall keyspace handle is a cheap Arc clone

`SingleWriterTxKeyspace` is `#[derive(Clone)]` over `Keyspace(Arc<KeyspaceInner>)`
(`keyspace/mod.rs:158`). `FjallStream` can hold a clone of the `events` handle and refill
itself — no `Arc<FjallStore>` threading, no `read_stream` signature change.

### Snapshots exist but must NOT be used here

fjall offers `db.snapshot()` / `read_tx()` → `Snapshot: Readable` for repeatable reads.
We deliberately avoid them: the `Iter` holds a `SnapshotNonce` that **pins fjall's GC
watermark while alive** — which is exactly why today's `FjallStream` eagerly loads then
drops the iterator (`stream.rs:16`: *"no LSM-tree iterator stays open while the stream is
held"*). A long-lived subscription holding a snapshot open would block compaction
indefinitely.

We don't need a snapshot anyway. The event store is **append-only with gapless per-stream
versions**: version N's bytes never change once written, and a stream's versions are a
contiguous `1..=N`. So keyset pagination (resume at `last_version + 1`) across independent
scans can only ever observe *more* events, never changed or missing ones. Immutability is
the snapshot. Re-scanning per batch preserves the no-GC-pinning property.

## Design

### The shared primitive: bounded scan + keyset resume

One read primitive, two termination rules:

| | source of next batch | termination |
|---|---|---|
| one-shot read (`read_stream`) | scan from `next_version`, `take(batch_size)` | batch shorter than `batch_size` → end of persisted data → `None` |
| subscription | same | shorter batch → park on the per-stream wake registry, then re-scan |

Both hold at most `batch_size` rows resident. A refill that returns exactly `batch_size`
rows is ambiguous (there may be more), so the cursor always attempts another scan; a
short or empty batch is the unambiguous end-of-data signal.

### `nexus-store` — the const + the in-memory adapter

- **`const MAX_BATCH: usize = 4096`** and **`const DEFAULT_BATCH: usize = 256`**, defined
  in `nexus-store` (the crate both adapters depend on) so the ceiling is shared. A
  `BatchSize` newtype validating `1..=MAX_BATCH` at construction carries the invariant by
  type rather than re-checking it in each builder.
- **`InMemoryStore`**: its `streams` map moves behind a shared handle so the returned
  `InMemoryStream` can re-read for refills (the store already holds the data; the stream
  needs read access to it). `read_stream` filters `version >= from` and `.take(batch_size)`.
  `InMemoryStream` keyset-resumes at `last_version + 1` when its buffer drains, terminating
  on a short batch. `InMemoryStore::with_batch_size(n)` sets the limit (rejecting
  `n > MAX_BATCH`); default `DEFAULT_BATCH`.

### `nexus-fjall` — store, stream, subscription, builder

- **`read_stream`**: `range(start..=end).take(self.batch_size)` instead of collecting the
  whole tail. Still loads the first batch eagerly (so the first `poll` is IO-free), then
  drops the `Iter` (no GC pin).
- **`FjallStream`** gains `{ events: SingleWriterTxKeyspace, id: OwnedStreamId,
  next_version, batch_size, done }`. When the `VecDeque` drains and `!done`, it re-scans
  `range(next_key..=end).take(batch_size)` synchronously (fjall scans are sync), advancing
  `next_version`. A batch shorter than `batch_size` sets `done`. `poll` returns `None` only
  when `done && empty`.
- **Subscription cursor** (`FjallSubscriptionStream`): unchanged in shape. It already
  drives a `FjallStream` via `poll_one` and parks on the wake registry when empty; now the
  inner `FjallStream` is itself bounded + paginating, so each catch-up cycle is capped.
- **`FjallStoreBuilder::batch_size(n)`**: stores the validated `BatchSize` on `FjallStore`;
  rejected at `open()` (or at the setter) if `n > MAX_BATCH`. Default `DEFAULT_BATCH`.
  `FjallStore` carries the `batch_size` so `read_stream` and `subscribe` both read it.

### Configuration values

- `DEFAULT_BATCH = 256` — resident cost ≈ `batch_size × avg row size` (256 × ~1 KiB ≈
  256 KiB), acceptable on mobile.
- `MAX_BATCH = 4096` — 16× headroom for server deployments while keeping the validated
  ceiling meaningful.

These are tunables, not load-bearing constants; changing them is a one-line edit.

## Error handling

- Oversized batch-size config is an **input validation error**, surfaced at the builder /
  setter — distinct from corruption or IO errors (per the crate's "input validation errors
  are not corruption errors" rule). fjall: a new `FjallError` variant or a builder-time
  `Result`. In-memory: `with_batch_size` returns `Result` (or takes a pre-validated
  `BatchSize`).
- Keyset-resume arithmetic (`last_version + 1`) already uses `Version::next()` →
  `VersionOverflow`; reused unchanged.
- Refill IO/corruption errors propagate as today: a yielded `Err` poisons the cursor.

## Testing (the 4 cross-cutting categories first)

1. **Sequence/Protocol** — append 10× `batch_size` events; one-shot `read_stream` yields all
   of them, in strict version order, across multiple internal refills. Subscription falls
   behind a writer pushing 10× `batch_size`; consumer drains correctly across refills.
2. **Lifecycle** — write > `batch_size` events, close, reopen, read: all events recovered
   across refills in order. (fjall only.)
3. **Defensive boundary** — `batch_size` of exactly `1`, exactly `MAX_BATCH`, and
   `MAX_BATCH + 1` (rejected at builder). Stream length of exactly `batch_size` (the
   ambiguous-refill boundary: must still terminate correctly, not hang or double-yield).
   Empty stream, single-event stream.
4. **Linearizability/Isolation** — concurrent writer appending while a reader paginates;
   assert the reader sees a monotonic, gap-free version sequence (immutability guarantee)
   and never a torn batch.

Then the standard methodologies. Property test: for random `stream_len` and `batch_size`
(strategies include `0, 1, MAX-1, MAX` boundaries), `read_stream` yields exactly
`stream_len` events in order regardless of how the chunk boundaries fall.

Each invariant is tested once in a canonical location; in-memory and fjall share the
behavioral assertions where the adapter difference is irrelevant.

## Documentation

- `RawEventStore::read_stream` doc: events arrive in chunks; the cursor refills internally;
  callers see the same `next()` contract.
- `Subscription::subscribe` doc: catch-up is bounded per cycle by the configured batch size.
- Both builders' `batch_size` setters: default, ceiling, and rejection behavior.

## Out of scope (restated)

- Writer backpressure.
- Per-event (iterator-held-open) streaming.
- All-streams / global-log bounded scan.
