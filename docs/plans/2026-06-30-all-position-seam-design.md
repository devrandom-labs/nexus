# `$all` Position Seam Redesign — adapter-defined cursor, global_seq out of the frame (design note)

**Status:** Design / pre-freeze contract decision. Feeds #213 (postgres adapter) and #204–#210 (freeze hardening). Supersedes the "defer the position widening" scoping in `docs/plans/2026-06-30-postgres-adapter-plan.md`.

**Author context:** Surfaced while designing the postgres adapter (#213). The ordering decision (Way 2 watermark, `project_postgres_adapter_decision`) exposed that `GlobalSeq` is a fjall-ism baked into the *shared* contract at three depths. This note proposes removing it from the shared layers and making the `$all` resume position adapter-defined.

---

## The problem (grounded in code)

`GlobalSeq` does **two unrelated jobs** under one type:

1. **A per-event stored scalar** — a `u64` in the frozen wire frame (`wire.rs:13`, offset 1, `HEADER_FIXED_SIZE = 19`) and on every `PersistedEnvelope` (`envelope.rs:332`).
2. **The `$all` subscription resume position** — `AllCatchup::Position = GlobalSeq`, derived from the envelope via `position_of(env) = env.global_seq()` (`catchup.rs:145`), advanced by the `+1` successor `GlobalSeq::next()` (`catchup.rs:149`).

Both are **fjall-isms hoisted into the shared contract**: fjall stamps a monotonic counter inside its single serialized write-tx and stores it inline, so insert-order == commit-order and a single scalar position works. A networked, concurrent adapter (postgres) breaks every part of that:

- Its position needs to be `(txid, global_seq)` (the Way-2 watermark, `project_postgres_adapter_decision`) — which **does not fit** the frame's 8-byte field and **has no `+1` successor**.
- Its `global_seq` (`BIGINT IDENTITY`) is assigned at insert but visible at commit, so a scalar `global_seq`-ordered cursor silently drops events (the #213 hazard).

So the contract as frozen would force every future adapter to either serialize writers (defeating the point of a server DB) or violate the contract.

### What actually depends on the per-event scalar

Tracing every use (grounded): the per-event `global_seq` exists almost entirely to serve job (2). Aggregate load uses per-stream `Version`. The CBOR backup box *deliberately strips* `global_seq` (store-local, re-stamped on import). fjall recovers it from its `events_global` **key** (`wire_key.rs:130`), not the frame — the frame copy is purely a redundant cross-check (`scan.rs:138`). So **nothing intrinsic requires `global_seq` in the shared frame.**

---

## Goal

Make the `$all` resume position **adapter-defined**, and remove the store-local `global_seq` from the **shared** wire frame and `PersistedEnvelope`. Each adapter assigns, stores, and surfaces its own position; `nexus-store` owns only the *abstraction*.

End state:
- **Wire frame / `PersistedEnvelope`**: carry only intrinsic event data — `version`, `event_type`, `schema_version`, `payload`, `metadata`. **No `global_seq`.**
- **Position**: an adapter-defined associated type on `RawEventStore`, surfaced *alongside* `$all` events (position-tagged stream items), with a small `nexus-store` trait as the bound.
- `GlobalSeq` (the `u64` newtype) **moves into `nexus-fjall`** as fjall's chosen position. `nexus-postgres` defines `(txid, seq)`. `nexus-store` keeps only the position trait.

Dependency direction makes this clean: `nexus-store` cannot reference its adapters, so the *concrete* position must live in the adapter and only the *trait* in the store — exactly the split above.

---

## Proposed seam

### 1. `nexus-store`: a position trait

```rust
/// Where an `$all` subscription resumes. Adapter-defined: a scalar for an
/// embedded store (fjall), a commit-ordered composite for a concurrent SQL
/// store (postgres), an LSN for a WAL tail. Carried alongside `$all` events,
/// never derived from the (position-free) envelope.
pub trait AllPosition: Copy + Ord + Send + Sync + core::fmt::Debug + 'static {}
```

`Ord` gives the loop "is this position after my checkpoint" without a successor function. (See the resume-model fork below for whether we also need a `next`/`after`.)

### 2. `RawEventStore`: associated position + position-tagged `$all`

```rust
type AllPosition: AllPosition;
type AllStream: futures::Stream<Item = Result<(Self::AllPosition, PersistedEnvelope), Self::Error>> + Send + 'static;

fn read_all(&self, from: Self::AllPosition)
    -> impl Future<Output = Result<Self::AllStream, Self::Error>> + Send;
```

Per-stream `read_stream` is **unchanged** — it stays keyed on `Version` (a single stream genuinely has a gapless successor sequence; no hazard there).

### 3. `Catchup` (`AllCatchup`): position from the tag, not the envelope

```rust
type Position = S::AllPosition;
// scan item is (Position, PersistedEnvelope) → position_of reads the tag
```

### 4. `Subscription`

```rust
pub fn subscribe_all(&self, from: Option<S::AllPosition>) -> ...
```

### 5. Wire frame: drop `global_seq`

`encode_frame`/`decode_frame` lose the `global_seq` arg/field; `HEADER_FIXED_SIZE` 19 → 11; `align_padding` recomputes the 16-byte payload boundary (the version byte + remaining fixed fields still precede it). Bump `FrameFormatVersion` (the #205 version byte exists for exactly this) — or, since no 1.0 data exists yet, revise V1 in place.

### 6. fjall

- Stamps + stores its position in the `events_global` key (already does); `type AllPosition = GlobalSeq` (the newtype *moves here*).
- Drops the redundant frame `global_seq` + the key/frame cross-check.
- `$all` scan tags each item with the key-derived position. Per-stream `read_stream` events no longer carry a global position (correct — global position is an `$all` concept).

### 7. postgres (#213)

- `type AllPosition = PgAllPos { txid: u64, seq: u64 }` (`Ord` = lexicographic).
- `read_all(from)` = the Way-2 watermark query, `ORDER BY txid, global_seq`, resume `WHERE (txid, global_seq) > (from.txid, from.seq)`. **Now fully correct** — the live `$all` subscription no longer skips, because resume is by the composite position, not a bare scalar.

---

## Forks to decide

**A. Sequencing.** Do the seam redesign *now* (prerequisite to postgres; freeze the good contract) vs. ship postgres on the thin contract and defer. → **Recommend now.** The whole point of #213 is to fix exactly this before freezing; deferring means freezing a contract we already know is wrong, then breaking it.

**B. Resume model: inclusive+successor vs. exclusive strict-after.** Today: `read_*` is INCLUSIVE and the loop derives strict-after via `next_pos` = `+1`. A tuple position has no `+1`. Two ways:
- B1 — Make `read_all` **exclusive** ("strictly after `from`"); the loop passes the last position directly (`WHERE pos > from`). Clean for tuples; matches SQL row-value comparison. Diverges `$all` from `read_stream`'s inclusive contract.
- B2 — Keep inclusive; the `AllPosition` trait provides a `successor`/`after`. Awkward for `(txid, seq)` (no natural successor in txid space).
→ **Recommend B1** for `$all` (exclusive, `Ord`-based), leaving `read_stream` inclusive on `Version`. Document the intentional asymmetry (CLAUDE rule 4).

**C. Frame change scope.** Dropping `global_seq` rewrites the stored row layout. Pre-freeze + the format-version byte make it safe; confirm we accept a frame change (no released 1.0 data). → **Recommend bump to frame V2** (cleaner than mutating V1) so any local pre-freeze data still decodes via the V1 branch during transition, then drop V1 before freeze.

**D. Naming.** `nexus-store` position trait name — `AllPosition`? `Cursor`? `Checkpoint`? The per-event scalar disappears entirely from the shared layer (not even a bare `u64`). → trait name TBD; `GlobalSeq` name stays with fjall.

---

## Cost / consequences (honest)

- **Bigger than an adapter:** touches `wire.rs`, `envelope.rs`, `catchup.rs`, `subscription.rs`, `store.rs` (RawEventStore), and `nexus-fjall` — a `nexus-store` core refactor with postgres as its validation. Per-stream path is untouched.
- **`read_stream` events lose `global_seq()`** — acceptable (global position is `$all`-only); audit no consumer relies on per-stream global position.
- **Consumer `$all` checkpoints become adapter-defined** and must be serializable (fjall: a `u64`; postgres: a pair). Document this in the `Subscription` API.
- **fjall loses the redundant key/frame cross-check** — minor; the key is authoritative.

---

## Suggested execution order

1. `nexus-store`: introduce `AllPosition` trait + `RawEventStore::AllPosition` + position-tagged `AllStream`; thread through `Catchup`/`subscription_cursor`/`Subscription`. (Keep `GlobalSeq` temporarily as fjall's position, re-exported, to stage the change.)
2. Drop `global_seq` from `wire.rs` + `PersistedEnvelope` (frame V2). Update fjall to source its position from the key and tag `$all` items.
3. Move `GlobalSeq` into `nexus-fjall`.
4. Write `nexus-postgres` (per the adapter plan, revised) with `PgAllPos` — and the live `$all` subscription is correct by construction, so the "skip-demonstration test + deferred follow-up" in the adapter plan is replaced by a real "no-skip under concurrent writers" linearizability test.
5. Findings → #204–#210.
