# nexus refactor: extract the subscription loop to nexus-store; fjall keeps only fjall

- **Date:** 2026-06-22
- **Status:** Design in progress, pending spec review
- **Branch:** `refactor/fjall-cursor-unification`
- **Scope:** Cross-crate. Extract the common catch-up-then-tail subscription machinery into
  `nexus-store`; reduce `nexus-fjall` to the irreducibly-fjall parts; plus the rule-fix
  cleanups in fjall. Stable-equivalent Rust features only (no nightly TAIT / `gen`).

## 0. Guiding principle (the whole point of this refactor)

`nexus-fjall` is **only the fjall implementation**. Any interface or behavior that is
common across storage adapters MUST live in `nexus-store`; `nexus-fjall` is left holding
only what is irreducibly fjall — its on-disk key byte layout, its `fjall::Slice` decode,
its transactions, its `fjall::Iter`.

The test for every type/function touched: *would a postgres adapter author write this same
code?* If yes, it is common → `nexus-store`. If it names `fjall::Iter`, `fjall::Slice`, or
the on-disk key bytes, it is fjall's.

This is not speculative generality. A postgres adapter is a real, named consumer
(`project_api_freeze_hardening`: "write postgres adapter before freezing the contract").
Extracting the common interface now is exactly how we get the 1.0 contract right with the
second adapter in view, rather than discovering the abstraction was fjall-shaped after the
freeze.

## 1. Motivation

`nexus-fjall` grew by copy-paste-and-tweak and currently holds **two** classes of code
that violate the principle in §0:

**(a) Common machinery hand-rolled inside the adapter.** The catch-up-then-live-tail
subscription loop — drain a bounded scan, register the waiter, reopen, park on a notifier —
is written **twice** in fjall (`subscription_stream.rs` for per-stream, keyed by `Version`;
`all_subscription_stream.rs` for `$all`, keyed by `GlobalSeq`), each carrying the same
subtle lost-wakeup discipline. None of this is fjall-specific: it is the universal
event-store read pattern (EventStoreDB `$all`, the Kafka consumer fetch loop, Postgres
logical replication all factor it identically — see §2.5). A postgres adapter would
re-hand-roll the identical loop and re-risk the identical lost-wakeup bug. **It belongs in
`nexus-store`.**

**(b) fjall's own bounded-cursor duplication.** The bounded scan cursor is also written
twice (`stream.rs` `FjallStream`, `all_stream.rs` `FjallAllStream`) — identical
`VecDeque<(Slice,Slice)>` + `refill()` + `poll_one()` shape, differing only in key-byte
encoding and `Slice` decode. This *is* fjall-specific (it touches `fjall::Iter`/`Slice`), so
it is collapsed **inside fjall** via a fjall-private strategy — it does not go to store.

Alongside both, several details contradict the project's own `CLAUDE.md` rules (§5).

## 2. Research findings (primary-sourced)

Per `CLAUDE.md` rule 0, every claim cites a source.

### 2.1 fjall's `range()` is already a lazy streaming cursor

Read from installed source (`fjall 3.1.4` → `lsm-tree 3.1.4`,
`~/.cargo/registry/src/.../fjall-3.1.4/`):

- `Keyspace::range<K, R>(&self, range: R) -> fjall::Iter` (`src/keyspace/mod.rs:444`).
- `fjall::Iter` = `{ iter: Box<dyn DoubleEndedIterator<Item = lsm_tree::IterGuardImpl> + Send + 'static>, nonce: SnapshotNonce }`
  (`src/iter.rs:7-20`) — **no lifetime parameter; fully owned; `Send + 'static`.** It can be
  stored in a struct and driven as a `Stream`.
- The inner iterator is `lsm_tree`'s `TreeIter` (`lsm-tree-3.1.4/src/range.rs:99-235`): a
  `self_cell` over a pinned `SuperVersion` + a k-way `Merger`/`MvccStream`; `next()` pulls
  the next LSM block from disk only when the current one drains.

**Implication:** `range()` holds a block cursor; it does not materialize rows. fjall's
`VecDeque` + `take(batch_size)` + re-scan loop is **redundant as a memory bound** — a single
owned `Iter` streams with bounded memory.

**The one real reason to close+reopen a scan:** a held `Iter` pins the GC watermark via its
`SnapshotNonce` for its lifetime; fjall docs warn to keep iterators short-lived
(`src/readable.rs:9-15`, `src/snapshot.rs:9-15`). So periodically reopening is justified
**only** for a never-ending subscription (to let GC advance), not for a bounded one-shot
read. Current fjall comments mis-attribute the loop to memory bounding.

### 2.2 Bounded reads gain snapshot consistency (behavior change)

Today each `refill()` opens a fresh `range`, so a long read can observe appends landing
mid-scan. A single owned `Iter` pins one `SnapshotNonce`, so a bounded read becomes
**repeatable-read / point-in-time consistent** as of read start — the stronger event-store
semantic, and consistent with keyset-resume correctness (use-the-index-luke, "No Offset":
keyset is "not sensitive to concurrent changes in lower values";
https://use-the-index-luke.com/no-offset).

### 2.3 Other fjall APIs

- `Snapshot`/`read_tx()` give a consistent cross-keyspace `Readable` view
  (`src/readable.rs:12-301`, `src/tx/single_writer/mod.rs:78-80`). fjall's writes already
  share one `write_tx`; confirm no multi-partition *read* spans two bare `get()` (rule 1).
- `WriteTransaction::fetch_update`/`update_fetch` (`src/tx/single_writer/write_tx.rs:181,240`):
  atomic single-key RMW. **Evaluated and rejected** for the `GlobalSeq` counter — the loop
  needs the counter value during per-event assignment; a running `u64` counter is clearer.
- No user-facing auto-increment/sequence exists; the store-global `GlobalSeq` counter is
  necessary and correct.
- `is_empty()` O(log n) vs `len()` O(n); docs warn against `len()==0`
  (`src/keyspace/mod.rs:539-575`). Use `is_empty()` if emptiness is ever checked.
- fjall 3 terminology: container = `Database`, column family = `Keyspace`.

### 2.4 Rust language features (toolchain: edition 2024, ~1.96 nightly)

- **RPIT / RPITIT** stable since 1.75
  (https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits/). This is what makes the
  design **zero-cost with no `Box<dyn>` on stable**:
  - `Subscription::subscribe(..) -> impl Stream<Item=…> + Send` (RPIT) hides the `unfold`
    state-machine type — there is no associated `Stream` type to name, so no boxing. This is
    only possible *because* the extraction deletes the associated-type `RawSubscription`
    trait (§1); the old design boxed precisely to name that associated type.
  - `WakeSource::arm(..) -> impl Future<Output=()> + Send + 'static` (RPITIT) hides the wait
    future — no `type Wait` associated type to box. RPITIT method futures are not
    `dyn`-compatible, which is fine: `WakeSource` is only ever a generic bound, never `dyn`.
- **`async gen` blocks** — not usable (pre-RFC; blocked on stable std `AsyncIterator`;
  https://github.com/rust-lang/rust/issues/117078). `futures::Stream` stays.
- **TAIT not needed.** It was the nightly fallback for naming the `unfold` type; RPIT/RPITIT
  achieve the same zero-cost result on stable, so no nightly feature (and no ICE risk) is
  taken on.
- Edition-2024 RPIT capture + `+ use<>` (1.82) — used to control exactly which generics the
  `impl Stream` return captures; delete legacy lifetime tricks.

### 2.5 The abstraction boundary (prior art)

EventStoreDB `$all`, the Kafka consumer fetch loop, and Postgres logical replication all
factor catch-up-then-tail identically: a **monotonic `Position`** + a
**`fetch(after) -> batch`** + a **"wait for more"** step that differs only in mechanism
(in-process notify vs server push / `LISTEN`)
(https://docs-next.eventstore.com/clients/rust/catch-up-subscriptions/;
https://www.confluent.io/blog/kafka-producer-and-consumer-internals-4-consumer-fetch-requests/;
https://www.postgresql.org/docs/current/protocol-replication.html). This is precisely the
seam we cut: `nexus-store` owns the loop + the `Position`-keyed fetch (already `RawEventStore`)
+ the wake interface; the adapter supplies the fetch impl and the wake mechanism. Keyset
resume must be **strict greater-than / gap-tolerant** (the `GlobalSeq` is monotonic-but-
gapped; gap *detection* must not be built on it — https://arxiv.org/pdf/2210.12955).

### 2.6 Lost-wakeup-safe wake, expressed generically

The fjall cursors close the lost-wakeup race with `tokio::sync::Notify` +
`Notified::enable()` before the read (`StreamNotifiers` module docs,
`crates/nexus-store/src/notify.rs:44-67`). `enable()` requires `Pin<&mut Notified>`, which
only the holder of the pinned future can call — so it **cannot be hidden behind a clean
adapter trait** without leaking tokio. The generic, leak-free equivalent is a
**generation / seen-version** wake: the receiver tracks the last version it saw and waits
for the value to change. `tokio::sync::watch::Receiver::changed()` is exactly this and is
lost-wakeup-safe by construction (it compares against the receiver's own last-seen version),
and its wait future is `'static` (https://docs.rs/tokio/latest/tokio/sync/watch/). This is
the mechanism the `WakeSource` in-process impl will use, and the contract a postgres impl
satisfies via `LISTEN/NOTIFY` + a position high-water mark.

## 3. Design

### 3.1 `nexus-store` — the extracted common machinery

New / changed in `nexus-store`:

**`WakeSource` trait (new).** The pluggable "wait until there may be new events" interface.
Adapter-agnostic; no fjall/tokio types in the signatures.

```rust
/// Adapter-provided wake mechanism for live subscriptions. Lets the generic
/// subscription cursor park until new events may exist instead of polling.
/// In-process adapters use the provided StreamNotifiers impl; distributed
/// adapters (postgres) implement this over LISTEN/NOTIFY or replication.
///
/// Only ever used as a generic bound (never `dyn`), so `arm` returns an
/// RPITIT future — no associated `Wait` type, no boxing.
pub trait WakeSource: Send + Sync + 'static {
    /// Arm a lost-wakeup-safe wait for events on `stream` (None = $all).
    ///
    /// CONTRACT: the returned future captures a "seen version" at the moment
    /// `arm` is called; awaiting it resolves once a wake is delivered *after*
    /// that point. A wake delivered between `arm` and the await is therefore
    /// NOT lost. Spurious wakes are permitted (the cursor re-scans and re-arms).
    /// The future is owned (`'static`) — e.g. an `async move` over a cloned
    /// `watch::Receiver` — so it carries no borrow of `self`.
    fn arm(&self, stream: Option<&[u8]>) -> impl Future<Output = ()> + Send + 'static;

    /// Signal that new events for `stream` (and $all) are durably committed.
    /// MUST be called by the adapter *after* commit.
    fn wake(&self, stream: &[u8]);
}
```

**`StreamNotifiers` becomes the in-process `WakeSource` impl.** It already lives in
`nexus-store` (`src/notify.rs`). It is evolved from `Notify` + `enable()` to a
generation/`watch`-based mechanism so `arm()` can return a `'static`, lost-wakeup-safe
`Wait` future with no `enable()` leakage (§2.6). This is a self-contained internal change to
`notify.rs` with its own test coverage; the public wake/subscribe surface is preserved or
adjusted minimally. **Risk-noted** (see §6) — `StreamNotifiers` was carefully tuned (#176).

**`Catchup` trait (new, store-side seam).** To write the loop body *once* while keeping it
zero-cost, the loop is generic over a `Catchup` that fuses "bounded scan after a position"
(`RawEventStore`) and "arm a wait" (`WakeSource`) for one target. Two compile-time impls —
per-stream and `$all` — so the one source loop monomorphizes into two branch-free state
machines. No runtime `dyn`, no enum-of-streams.

```rust
pub(crate) trait Catchup: Send {
    type Position: Copy + Send;
    type Scan: Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send;
    type Error: core::error::Error + Send + Sync + 'static;
    fn read_after(&self, pos: Self::Position)
        -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;     // RawEventStore
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;              // WakeSource
    fn position_of(env: &PersistedEnvelope) -> Self::Position;
    fn start(from: Option<Self::Position>) -> Self::Position;
}
// StreamCatchup<S> { store: Arc<S>, id }  — Scan = S::Stream,    arm = store.arm(Some(id))
// AllCatchup<S>    { store: Arc<S> }       — Scan = S::AllStream, arm = store.arm(None)
// both for S: RawEventStore + WakeSource
```

**The generic loop (new).** One function `fn live<C: Catchup>(c: C, from) -> impl Stream<…> + Send`,
the single copy of the catch-up-then-tail state machine. RPIT hides the `unfold` type — no Box:

```text
live(c, from) -> impl Stream:                 // body: futures::stream::unfold(...)
  let mut pos = C::start(from);
  loop {
    let mut scan = c.read_after(pos).await?;   // bounded scan
    let mut n = 0;
    while let Some(env) = scan.next().await {
      pos = C::position_of(&env); yield env;
      if (n += 1) == CATCHUP_CHUNK { break }   // drop+reopen to release the GC nonce
    }
    let caught_up = scan exhausted (not chunk-break);
    drop(scan);
    if caught_up {
      let wait = c.arm();                       // arm BEFORE the confirming re-scan
      let mut probe = c.read_after(pos).await?;
      if probe.next().await.is_none() { wait.await }  // park only if truly empty
      // else: loop; next iteration drains from pos
    }
  }
```

- `CATCHUP_CHUNK` is a `nexus-store` `const` (proposed 1024): the reopen granularity that
  bounds how long any single adapter scan (and its GC nonce) is held during a large backlog.
  The *only* surviving role of the old fjall `batch_size`, now owned by the layer that owns
  the loop.
- The lost-wakeup discipline (arm-before-confirm-rescan) exists in exactly one place, for
  every adapter, forever.

**`Subscription<S>` becomes a blanket builder, returning `impl Stream`.** For
`S: RawEventStore + WakeSource`:

```rust
pub fn subscribe(&self, id: &impl Id, from: Option<Version>)
    -> impl Stream<Item = Result<PersistedEnvelope, S::Error>> + Send + use<S>;
pub fn subscribe_all(&self, from: Option<GlobalSeq>)
    -> impl Stream<Item = Result<PersistedEnvelope, S::Error>> + Send + use<S>;
```

Both build the right `Catchup` and return `live(..)`. They are **infallible** (not `async`,
no `Result`): a scan-open failure surfaces as the first `Err` item in the stream, so errors
flow through one uniform channel. The adapter-implemented `RawSubscription` /
`RawAllSubscription` traits are **deleted** — adapters no longer write a subscribe method.

**`RawEventStore` gains nothing new conceptually** — `read_stream(id, from)` /
`read_all(from)` already are the bounded `Position`-keyed catch-up scan the `Catchup` impls
call.

### 3.2 `nexus-fjall` — only fjall

After extraction, `nexus-fjall` source:

```
store.rs        FjallStore + RawEventStore + WakeSource impls. append() decomposed (§5).
                WakeSource forwards to the held StreamNotifiers; append wakes after commit.
scan.rs    NEW  fjall-PRIVATE ScanStrategy trait + StreamScan/GlobalScan impls + the ONE
                bounded ScanCursor<S> (wraps a single lazy fjall::Iter, impl Stream).
wire_key.rs     (renamed encoding.rs) keys only: event key, global key, stream version.
                decode_event_value stays (real production read path).
snapshot.rs     snapshot value codec (feature = "snapshot"), pulled out of encoding.rs.
builder.rs      batch_size field + setter REMOVED.
partition.rs    unchanged.
error.rs        DecodeError loses the fake-sentinel branch (§5).
```

**Deleted from fjall:** `stream.rs`, `all_stream.rs`, `subscription_stream.rs`,
`all_subscription_stream.rs`. The first two collapse into `scan.rs`; the last two are gone
entirely — that loop now lives in `nexus-store`.

**fjall-private `ScanStrategy`** (`pub(crate)`, never exported): the only varying parts of
fjall's *own* bounded scan.

```rust
pub(crate) trait ScanStrategy: Send {
    type Position: Copy + Send + 'static;                 // Version | GlobalSeq
    fn lower_key(&self, from: Self::Position) -> Vec<u8>; // fjall key bytes
    fn upper_key(&self) -> Vec<u8>;
    fn decode(&self, key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError>;
}
```
- `StreamScan { id, label }` → `Position = Version`; event-key codec; today's `decode_row`.
- `GlobalScan` → `Position = GlobalSeq`; global-key codec; today's `decode_global_row`
  (keeps the global_seq key-vs-frame cross-check, rule 4).

**`ScanCursor<S>`** wraps one lazy `fjall::Iter` from `keyspace.range(lower..=upper)` and
maps each `Guard` through `strategy.decode`. No `VecDeque`, no `refill`, no `batch_size`, no
`done`. `read_stream`/`read_all` return `ScanCursor<StreamScan>` / `ScanCursor<GlobalScan>`,
which remain the `RawEventStore::Stream` / `::AllStream` associated types.

**`WakeSource` for `FjallStore`**: `arm`/`wake` forward to the `Arc<StreamNotifiers>` it
already holds; `append` calls `wake` after the durable commit (as it does today).

### 3.3 `append` decomposition + arithmetic rule fixes (fjall)

Split `append` into `read_current_version` / `check_optimistic` / `validate_sequential` /
write-loop; drop `#[allow(clippy::too_many_lines)]`. The three
`u64::try_from(i).unwrap_or(u64::MAX)` sentinels (rule 2 violations) are deleted by using
running `u64` counters advanced with `checked_add(1)` — no `usize -> u64` cast remains and
the `batch_len` computation disappears.

### 3.4 Encoding cohesion, fake error, encapsulation (fjall)

- **Fake sentinel removed:** `decode_event_value`'s
  `InvalidSize { expected: usize::MAX, actual: 0 }` guard and its redundant UTF-8 pre-check
  are deleted (`PersistedEnvelope::try_new` already validates `event_type` UTF-8).
- **`pub mod` leakage fixed:** `lib.rs` switches to private `mod` + curated `pub use` of only
  the real public surface (`FjallStore`, `FjallStoreBuilder`, `FjallError`, `KeyspaceConfig`,
  and the `ScanCursor` read-stream types named in public return positions).
- **Test-only `encode_event_value` deleted:** it is `pub` production code consumed only by
  tests/benches (`append` uses `wire::encode_frame`). Callers migrate to the real
  `wire::encode_frame` + `nexus_store::value` path; this also makes the benches measure
  production code (rule 8).

### 3.5 `batch_size` removed from the fjall public API

The `batch_size` field + `.batch_size(...)` setter leave `FjallStoreBuilder`. Memory
bounding is now the lazy `Iter`'s job; catch-up chunking is `nexus-store`'s `CATCHUP_CHUNK`.

## 4. Behavior changes (explicit)

1. **Bounded reads are now snapshot-consistent** (repeatable-read as of read start) instead
   of observing concurrent appends mid-scan (§2.2). Stronger semantic; the linearizability
   test still passes (asserts gap-free/monotonic + a fresh post-join read sees all).
2. **`batch_size` removed from `FjallStoreBuilder`** (public API change; acceptable pre-1.0).
3. **`RawSubscription` / `RawAllSubscription` deleted** from `nexus-store`'s adapter surface,
   replaced by the `RawEventStore + WakeSource` blanket builder (contract change on the
   freeze surface — intentional, done with postgres in view per §0).
4. **`Subscription::subscribe` / `subscribe_all` signature change**: from
   `async fn(..) -> Result<S::Stream, S::Error>` to a synchronous
   `fn(..) -> impl Stream<Item = Result<…, S::Error>> + Send` (zero-cost RPIT, no `Box`).
   Errors move in-band (first `Err` item) instead of at construction.
5. **`StreamNotifiers` internal mechanism** changes (Notify+enable → generation/watch) so
   `arm` can return an owned `'static` future. No change to its "wake routing per stream,
   reaped at zero" contract.

No on-disk wire-format change.

## 5. Testing impact

- The fjall subscription-cursor tests (`subscription_drains_many_batches...`,
  `subscribe_all_catches_up...`) move with the loop into `nexus-store`, retargeted at the
  generic `LiveCursor` driven by `InMemoryStore` (which gains a `WakeSource` impl) — so the
  loop is tested once, adapter-independently. fjall keeps an integration-level subscription
  smoke test proving its `RawEventStore + WakeSource` wiring drives the store-side loop.
- fjall bounded-read "pagination across refills" tests lose their `batch_size(4)` knob and
  become plain full-read correctness tests (the lazy `Iter` makes refills an implementation
  detail). Keep them; drop the knob.
- The generic loop's catch-up reopen cycles are exercised by seeding `> CATCHUP_CHUNK`
  events against `InMemoryStore`.
- Mandatory 4 categories (`CLAUDE.md` rule 7):
  - **Sequence/Protocol:** drain → reopen → arm → park → wake → drain, in `nexus-store`.
  - **Lifecycle:** write/close/reopen recovery (fjall); long bounded read is
    snapshot-consistent across a concurrent append (fjall).
  - **Defensive boundary:** corrupt key / value / global_seq mismatch per fjall
    `ScanStrategy::decode`; `WakeSource` lost-wakeup race (store, multi-thread).
  - **Linearizability:** concurrent reader/writer (fjall) asserting snapshot consistency;
    `WakeSource` concurrent arm/wake-not-lost test (store, ported from the current
    `concurrent_wake_is_not_lost`).
- `ScanStrategy::decode` unit-tested per impl with hand-built rows (preserves the current
  `decode_row`/`decode_global_row` tests).

## 6. Risks

- **`StreamNotifiers` mechanism change** is the load-bearing risk. The current
  enable-before-read design was a deliberate lost-wakeup fix (#176). The generation/`watch`
  replacement must be proven lost-wakeup-safe with the ported multi-thread concurrency test
  *before* the generic loop depends on it. Mitigation: change `notify.rs` + its tests as the
  first phase, in isolation, and only then build the loop on it.
- **Deleting `RawSubscription`/`RawAllSubscription`** touches the 1.0 freeze surface and any
  external adapter stubs. Acceptable per §0 (intentional, postgres-driven), but it is a
  breaking contract change to call out in the changelog.
- Holding one `fjall::Iter` across a `CATCHUP_CHUNK`-bounded drain pins the GC nonce for that
  chunk only; `CATCHUP_CHUNK` trades GC-pressure vs reopen overhead. 1024 is a starting
  point, tunable later if profiling shows pressure.

## 7. Out of scope / follow-ups

- Stack-allocated (`ArrayVec`) fjall scan keys instead of `Vec<u8>` per scan-open.
- Bulk import/export (#145/#220) via `Keyspace::start_ingestion`.
- A postgres `WakeSource` impl (the consumer that validates this extraction; separate repo
  / milestone).

## 8. Open questions

None outstanding. `batch_size` removal, the snapshot-consistency change, and placing the
loop + wake interface in `nexus-store` (deleting the adapter subscription traits) were
decided during design.
