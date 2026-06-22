# nexus-fjall refactor: cursor unification + lazy-Iter correction

- **Date:** 2026-06-22
- **Status:** Design approved, pending spec review
- **Branch:** `refactor/fjall-cursor-unification`
- **Scope:** Approach 2 — structural unification + the lazy-`Iter` correction + rule-fix cleanups + encapsulation. Stable-equivalent Rust features only (no nightly TAIT / `gen`).

## 1. Motivation

`nexus-fjall` grew by copy-paste-and-tweak. The crate expresses **two** behaviors —
a *bounded keyset-scan cursor* and a *live-tailing subscription cursor* — but each is
written **twice**, once for a single stream (keyed by `Version`) and once for `$all`
(keyed by `GlobalSeq`):

| Behavior | single-stream copy | `$all` copy |
|---|---|---|
| bounded scan cursor | `stream.rs` `FjallStream` | `all_stream.rs` `FjallAllStream` |
| live subscription cursor | `subscription_stream.rs` | `all_subscription_stream.rs` |

The two bounded cursors are byte-for-byte the same shape (`VecDeque<(Slice,Slice)>` +
`keyspace` + cursor + `batch_size` + `done` + `poisoned`, identical `refill()` /
`poll_one()` / `impl Stream`). They differ only in (a) scan-key encoding and (b) how a
row decodes into a `PersistedEnvelope`. The two live cursors are likewise identical
`futures::stream::unfold` loops carrying the same hard-won "register the waiter before
refill" lost-wakeup discipline — duplicated, so it must be kept correct in two places.

Alongside the duplication, several details contradict the project's own
`CLAUDE.md` rules (enumerated in §4–§6).

## 2. Research findings (primary-sourced)

Per `CLAUDE.md` rule 0, every claim below cites a source.

### 2.1 fjall's `range()` is already a lazy streaming cursor

Read from the installed source (`fjall 3.1.4` → `lsm-tree 3.1.4`,
`~/.cargo/registry/src/.../fjall-3.1.4/`):

- `Keyspace::range<K, R>(&self, range: R) -> fjall::Iter`
  (`fjall-3.1.4/src/keyspace/mod.rs:444`).
- `fjall::Iter` is `{ iter: Box<dyn DoubleEndedIterator<Item = lsm_tree::IterGuardImpl> + Send + 'static>, nonce: SnapshotNonce }`
  (`fjall-3.1.4/src/iter.rs:7-20`) — **no lifetime parameter; fully owned; `Send + 'static`.**
- The inner iterator is `lsm_tree`'s `TreeIter` (`lsm-tree-3.1.4/src/range.rs:99-235`): a
  `self_cell` over a pinned `SuperVersion` + a k-way `Merger`/`MvccStream`. `next()`
  advances the merge by one element and pulls the next LSM block from disk only when a
  reader's current block drains.

**Implication:** `range()` holds a cursor over LSM blocks; it does **not** materialize
rows. The adapter's `VecDeque` + `take(batch_size)` + re-scan-with-advanced-key loop is
**redundant as a memory bound** — a single owned `Iter` streams the same rows with
bounded memory.

**The one real reason to close+reopen a scan:** a held `Iter` pins the GC watermark via
its `SnapshotNonce` for its whole lifetime; fjall's docs warn to "keep iterators /
transactions short-lived" (`fjall-3.1.4/src/readable.rs:9-15`,
`src/snapshot.rs:9-15`). So periodically reopening the scan is justified **only** for the
never-ending subscription cursor (to let GC advance), **not** for a bounded one-shot
read. The current comments mis-attribute the loop to memory bounding.

### 2.2 Bounded reads gain snapshot consistency (one behavior change)

Today each `refill()` opens a *fresh* `range`, so a long read can observe appends that
land mid-scan. A single owned `Iter` pins one `SnapshotNonce`, so a bounded read becomes
**repeatable-read / point-in-time consistent** as of when the read began. This is the
stronger event-store semantic and is consistent with keyset-resume correctness
(use-the-index-luke, "No Offset": keyset is "not sensitive to concurrent changes in lower
values"; https://use-the-index-luke.com/no-offset).

### 2.3 Other fjall APIs

- `Snapshot` / `read_tx()` give a consistent cross-keyspace `Readable` view
  (`fjall-3.1.4/src/readable.rs:12-301`, `src/tx/single_writer/mod.rs:78-80`). The
  adapter's writes already share one `write_tx`; no multi-partition *read* currently
  spans two bare `get()` calls, so rule 1 is satisfied. (Confirm during implementation.)
- `WriteTransaction::fetch_update` / `update_fetch`
  (`fjall-3.1.4/src/tx/single_writer/write_tx.rs:181,240`) do atomic single-key RMW in a
  closure. **Evaluated and rejected** for the `GlobalSeq` counter: the loop needs the
  counter value *during* per-event assignment, not a read-write-once. A running `u64`
  counter is clearer (§4).
- No user-facing auto-increment/sequence exists — the store-global `GlobalSeq` counter is
  necessary and correct (internal `SeqNo` is the engine's MVCC clock, not a substitute).
- `is_empty()` is O(log n); `len()` is O(n) and docs warn against `len()==0`
  (`fjall-3.1.4/src/keyspace/mod.rs:539-575`). Use `is_empty()` if emptiness is ever
  checked.
- fjall 3 terminology: container = `Database`, column family = `Keyspace`. The adapter's
  "partition" = a fjall `Keyspace`. No rename forced; new docs should use 3.x terms.

### 2.4 Rust language features (toolchain: edition 2024, ~1.96 nightly)

- **AFIT / RPITIT** stable since 1.75
  (https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits/). The store traits
  already use native `async fn`; nothing to win, nothing to change.
- **`async gen` blocks** — the natural `unfold` replacement — are **not usable**: pre-RFC,
  double-gated (`gen_blocks` + `async_iterator`), and blocked on a stable std
  `AsyncIterator` (https://github.com/rust-lang/rust/issues/117078,
  https://areweasyncyet.rs/). `futures::Stream` stays.
- **TAIT** (`type_alias_impl_trait`, https://github.com/rust-lang/rust/issues/63063)
  could name the `unfold` type to drop the `Box<dyn Stream>`, but it is nightly and the
  project has documented incremental-build ICE history. **Rejected** for this refactor;
  the live cursor keeps `Pin<Box<dyn Stream + Send>>` (one heap alloc *per subscription*,
  not per poll).
- Stable wins to adopt: edition-2024 RPIT capture rules + `+ use<>` (1.82) let us delete
  any legacy lifetime-capture tricks.

### 2.5 The abstraction boundary (prior art)

EventStoreDB `$all`, the Kafka consumer fetch loop, and Postgres logical replication all
factor catch-up-then-tail identically: a **monotonic `Position` type** + a
**`fetch(after: Position, limit) -> batch`** function; live-tailing is the same fetch
re-issued when the batch is empty, differing only in the "wait for more" step
(https://docs-next.eventstore.com/clients/rust/catch-up-subscriptions/;
https://www.confluent.io/blog/kafka-producer-and-consumer-internals-4-consumer-fetch-requests/;
https://www.postgresql.org/docs/current/protocol-replication.html). redb and fjall both
prove the "one scan-iterator type, key-layout-in-a-trait" pattern
(https://docs.rs/redb/latest/redb/struct.Table.html). Keyset resume must be **strict
greater-than / gap-tolerant** (the `GlobalSeq` is monotonic-but-gapped; gap *detection*
must not be built on it — https://arxiv.org/pdf/2210.12955).

This is exactly the shape we extract: an associated `Position` type + keyset bound
encoders + a row decoder, in one trait, with two compile-time impls (so monomorphization
is bounded and the hot path stays branch-free — associated types, not fn-pointers or
enum-dispatch).

## 3. Design

### 3.1 File layout (9 source files → 7)

```
store.rs        FjallStore struct + RawEventStore + RawSubscription + RawAllSubscription
                impls. append() decomposed into named steps.
scan.rs    NEW  ScanStrategy trait; StreamScan + GlobalScan impls; the ONE bounded
                ScanCursor<S> (impl futures::Stream).
live.rs    NEW  the ONE LiveCursor<S> + Park policy. Replaces subscription_stream.rs and
                all_subscription_stream.rs. Holds the single copy of the
                enable-before-refill lost-wakeup loop. OwnedStreamId moves here.
wire_key.rs     (renamed from encoding.rs) keys only: event key, global key, stream
                version codecs. decode_event_value stays (real production read path).
snapshot.rs     snapshot value codec (feature = "snapshot"), pulled out of encoding.rs.
builder.rs      batch_size field + setter removed; otherwise unchanged.
partition.rs    unchanged.
error.rs        DecodeError loses the fake-sentinel branch (§5).
```

Deleted: `stream.rs`, `all_stream.rs`, `subscription_stream.rs`,
`all_subscription_stream.rs`.

### 3.2 `ScanStrategy` — the varying part, extracted once

```rust
/// The part of a keyset scan that differs between the per-stream and $all
/// reads: the resume position type, the keyset bound encoding, and how a
/// stored row decodes into a PersistedEnvelope.
pub(crate) trait ScanStrategy: Send {
    /// Resume cursor type. `Version` for a single stream, `GlobalSeq` for $all.
    type Position: Copy + Send + 'static;

    /// Keyset lower bound key for a scan starting at (inclusive) `from`.
    /// "Strictly after P" is expressed by the caller passing `P.next()`.
    fn lower_key(&self, from: Self::Position) -> Vec<u8>;

    /// Keyset upper bound key (end of this strategy's key range).
    fn upper_key(&self) -> Vec<u8>;

    /// Decode one (key, value) row into an envelope, mapping every malformed
    /// shape to a typed FjallError. Owns the per-strategy validation
    /// (per-stream label; $all global_seq cross-check).
    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError>;

    /// Resume position to scan after, given the last yielded envelope.
    fn advance(env: &PersistedEnvelope) -> Self::Position;
}
```

Two impls:

- `StreamScan { id: OwnedStreamId, label: ArrayString<64> }` — `Position = Version`; keys
  via the event-key codec; `decode` = today's `decode_row`; `advance` = `env.version()`.
- `GlobalScan` — `Position = GlobalSeq`; keys via the global-key codec; `decode` =
  today's `decode_global_row` (keeps the global_seq key-vs-frame cross-check, rule 4);
  `advance` = `env.global_seq()`.

The shared envelope-construction tail (`PersistedEnvelope::try_new` + error mapping) is a
free helper used by both `decode` impls.

`Vec<u8>` keys: one small allocation per *scan open* (not per row), off the hot path.
Acceptable; a stack `ArrayVec`/`Id::BYTE_LEN`-bounded optimization is a possible
follow-up, not part of this refactor.

### 3.3 `ScanCursor<S>` — the one bounded cursor

```rust
pub struct ScanCursor<S: ScanStrategy> {
    iter: fjall::Iter,        // single lazy LSM cursor; pins one snapshot nonce
    strategy: S,
    poisoned: bool,           // once an error is yielded, subsequent polls -> None
    #[cfg(debug_assertions)]
    prev: Option<S::Position>,// monotonicity assertion (debug only)
}
```

- Built by `read_stream`/`read_all`: `keyspace.range(lower..=upper)` once, then map each
  `Guard` through `strategy.decode`.
- No `VecDeque`, no `refill()`, no `batch_size`, no `done` (the `Iter` returns `None`).
- `impl futures::Stream` synchronously (`Poll::Ready(self.poll_one())`), preserving the
  current model. On `decode` error: poison and yield the error once.

`read_stream` / `read_all` return `ScanCursor<StreamScan>` / `ScanCursor<GlobalScan>`.
These remain the `RawEventStore::Stream` / `::AllStream` associated types.

### 3.4 `LiveCursor<S>` + `Park` — the one live cursor

```rust
/// Where a caught-up live cursor parks for the next wake.
pub(crate) trait Park: Send {
    fn notifier(&self) -> &Arc<tokio::sync::Notify>;
}
struct StreamPark { guard: SubscriptionGuard }    // per-stream wake entry; reaped on drop
struct AllPark    { all: Arc<tokio::sync::Notify> } // store-wide $all notifier
```

`LiveCursor::new<S, P>(store, strategy, park, start)` builds the **single** `unfold` loop:

```text
loop {
  if let Some(item) = scan.next() { advance `start`; yield item }   // drain current chunk
  // chunk drained: register the waiter BEFORE reopening (lost-wakeup discipline)
  let notify = Arc::clone(park.notifier());
  let notified = notify.notified(); pin!(notified); notified.enable();
  scan = ScanCursor::open(store, &strategy, start);                 // reopen from resume pos
  if scan.is_empty() { notified.await }                             // truly caught up: park
}
```

- During catch-up the cursor reopens its `ScanCursor` every **`CATCHUP_CHUNK`** rows (an
  internal `const` — see §3.6) so it does not pin the GC watermark across an unbounded
  backlog. Reopening releases the prior `Iter`'s nonce.
- Return type stays `Pin<Box<dyn futures::Stream<Item = Result<PersistedEnvelope, FjallError>> + Send>>`
  (TAIT rejected, §2.4). One heap alloc per subscription.
- The enable-before-refill comment + reasoning now live in exactly one place.

`StreamPark` carries the `SubscriptionGuard` (keeps the per-stream wake entry registered,
reaped on drop); `AllPark` parks on the always-present store-wide notifier.

### 3.5 `append` decomposition + arithmetic rule fixes

`append` splits into named private steps on `FjallStore`:

- `read_current_version(&tx, id) -> Result<u64, AppendError>` — point-read + corrupt
  handling (returns 0 for a new stream).
- `check_optimistic(current, expected, id) -> Result<(), AppendError>` — the OCC check.
- `validate_sequential(current, envelopes, id) -> Result<(), AppendError>` — sequential
  version validation.
- the write loop.

Drop `#[allow(clippy::too_many_lines)]`. The three
`u64::try_from(i).unwrap_or(u64::MAX)` sentinels (rule 2 violations) are **deleted** by
switching from enumerate-index arithmetic to running `u64` counters advanced with
`checked_add(1)` — there is no `usize -> u64` cast left, and the `batch_len` computation
disappears (`new_global` is simply the last assigned `global_seq`).

### 3.6 `batch_size` removed from the public API

- Remove the `batch_size` field and `.batch_size(...)` setter from `FjallStoreBuilder`.
- The live cursor's catch-up chunk becomes an internal `const CATCHUP_CHUNK: usize`
  (proposed 1024) in `live.rs`. Documented as the GC-release / wakeup-latency granularity.
- Bounded reads ignore chunking entirely (single lazy `Iter`).
- `BatchSize` (from `nexus-store`) is no longer referenced by the fjall builder. (It may
  still be used elsewhere; this refactor only stops fjall from exposing it.)

### 3.7 Encoding cohesion, fake error, encapsulation

- **Fake sentinel removed:** `decode_event_value`'s
  `InvalidSize { expected: usize::MAX, actual: 0 }` guard (the unreachable `u32 -> usize`
  branch) is deleted along with its redundant UTF-8 pre-check — `PersistedEnvelope::try_new`
  already validates `event_type` UTF-8. `decode_event_value` then returns the decoded
  offsets directly from `wire::decode_frame`.
- **`pub mod` leakage fixed:** `lib.rs` switches to private `mod` declarations + a curated
  `pub use` exposing only the real public surface: `FjallStore`, `FjallStoreBuilder`,
  `FjallError`, `KeyspaceConfig`, and the cursor types named in public return positions
  (`ScanCursor<StreamScan>`, `ScanCursor<GlobalScan>`, and the two live-cursor types — or
  type aliases for them).
- **Test-only `encode_event_value` deleted:** it is `pub` production code consumed only by
  tests and benches (`append` itself uses `wire::encode_frame`). Its callers migrate to
  the real `wire::encode_frame` + `nexus_store::value` newtype path. This also fixes the
  benches to measure production code (rule 8).

## 4. Behavior changes (call out explicitly)

1. **Bounded reads are now snapshot-consistent** (repeatable-read as of read start) instead
   of picking up concurrent appends mid-scan (§2.2). Stronger semantic; the
   linearizability test still passes (it asserts gap-free/monotonic + a fresh post-join
   read sees all).
2. **`batch_size` is removed from `FjallStoreBuilder`** (public API change; acceptable
   pre-1.0). Catch-up chunking is internal.

No on-disk wire-format change. No change to `RawEventStore` / `RawSubscription` trait
contracts.

## 5. Testing impact

- Existing bounded-read "pagination across refills" tests (`read_yields_all_across_refills`,
  `read_terminates_at_exact_batch_boundary`, `read_from_midpoint_resumes_across_refills`,
  `read_reopened_store_recovers_all_across_refills`) lose their `batch_size(4)` knob. They
  become plain full-read correctness tests (still valid; the lazy `Iter` makes "refills" an
  implementation detail with nothing to page). Keep them, drop the knob.
- The live-cursor catch-up re-open cycles were exercised by tiny `batch_size`. To keep
  exercising multiple reopen cycles cheaply with a fixed internal `CATCHUP_CHUNK`, seed
  past the constant (e.g. `CATCHUP_CHUNK + a few` events) in the relevant subscription
  tests. The existing `subscription_drains_many_batches_then_sees_live_event` keeps its
  intent by seeding `> CATCHUP_CHUNK`.
- New / retained coverage in the mandatory 4 categories (`CLAUDE.md` rule 7):
  - **Sequence/Protocol:** open scan → drain → reopen (live) → park → wake → drain.
  - **Lifecycle:** write/close/reopen recovery (retained); snapshot-consistency of a long
    bounded read across a concurrent append.
  - **Defensive boundary:** corrupt key / corrupt value / global_seq mismatch per strategy
    (`decode` impls) — retained, routed through the unified `ScanStrategy::decode`.
  - **Linearizability:** the concurrent reader/writer test, now asserting snapshot
    consistency (gap-free monotonic prefix) under a single `Iter`.
- `ScanStrategy::decode` unit-tested directly per impl with hand-built rows (preserves the
  current `decode_row` / `decode_global_row` unit tests).

## 6. Out of scope / follow-ups

- Stack-allocated (`ArrayVec`) scan keys instead of `Vec<u8>` per scan-open.
- Migrating bulk import/export (#145/#220) to `Keyspace::start_ingestion`.
- TAIT to drop the live cursor's `Box<dyn>` (revisit when stable).
- Switching to `OptimisticTxDatabase` (would add fjall's `Conflict` as a retryable source;
  explicitly *not* wanted — `CLAUDE.md` rule 5 keeps conflicts surfaced, not retried).

## 7. Open questions

None outstanding. `batch_size` removal and the snapshot-consistency behavior change were
decided during design.
