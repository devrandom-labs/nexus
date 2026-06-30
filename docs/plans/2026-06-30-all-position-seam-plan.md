# `$all` Position Seam Redesign Implementation Plan (#266)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the `$all` resume position adapter-defined and remove store-local `global_seq` from the shared wire frame + `PersistedEnvelope`, so a concurrent SQL adapter (postgres, #213) is correct by construction — before freezing #204–#210.

**Architecture:** `nexus-store` owns only an `AllPosition` trait + `RawEventStore::AllPosition` associated type; the `$all` read yields **position-tagged** items `(AllPosition, PersistedEnvelope)`; the generic live loop threads the position from the tag (no `position_of(env)`); `subscribe_all` hands the consumer `(AllPosition, PersistedEnvelope)` for checkpointing; `global_seq` leaves the frame (V2) and the envelope; `GlobalSeq` moves into `nexus-fjall`. Per-stream `Version` path is untouched.

**Tech Stack:** Rust 2024, `futures::Stream`, the existing `Catchup`/`subscription_cursor` machinery.

**Design note (rationale + forks, all locked):** `docs/plans/2026-06-30-all-position-seam-design.md`. **Locked decisions:** position adapter-defined; `$all` resume **exclusive** (`Ord`, no successor); frame **V2** (drop `global_seq`); `read_stream` envelopes drop `global_seq()` (signed off); trait named `AllPosition`.

---

## Grounded seam map (verified against code)

| Layer | File:line | Change |
|---|---|---|
| Position trait | new in `store.rs` | add `AllPosition: Copy + Ord + Send + Sync + Debug + 'static` |
| `RawEventStore` | `store.rs:117` | add `type AllPosition`; `read_all(from: Self::AllPosition)`; `AllStream::Item = Result<(Self::AllPosition, PersistedEnvelope), Error>` |
| `Store<S>` delegation | `store.rs` (Store fwd impl) | forward `AllPosition`/`AllStream`/`read_all` to inner |
| Catchup | `catchup.rs:129-156` | `AllCatchup::Position = S::AllPosition`; scan item becomes `(Position, env)`; **exclusive** resume (drop `+1` successor) |
| Live loop | `subscription_cursor.rs:51-135` | thread position from the tagged item; yield `(Position, env)`; delete `position_of` reliance |
| Subscription | `subscription.rs:115` | `subscribe_all(from: Option<S::AllPosition>) -> Stream<Item=Result<(S::AllPosition, PersistedEnvelope), E>>` |
| Wire frame | `wire.rs:13,67,73,555,588` | drop `global_seq`; `HEADER_FIXED_SIZE` 19→11; frame **V2** |
| Envelope | `envelope.rs:332,360,422,577` | drop `global_seq` field, `try_new` param, `global_seq()`, `for_decode` stamp |
| `GlobalSeq` | `store.rs:346` | **move to `nexus-fjall`** as its `AllPosition` |
| fjall | `wire_key.rs:130`, `scan.rs`, `store.rs` | source position from `events_global` key; tag `$all` items; drop frame `global_seq` + cross-check |

---

## Resume-model change (the conceptual core)

Today (`catchup.rs`): `read_from` INCLUSIVE; loop derives next position as `next_pos(position_of(env))` = `+1` successor. A tuple position has no `+1`.

New: `$all` resume is **exclusive** — the loop reopens with the last-delivered position and the adapter reads `WHERE pos > from`. The `Catchup` seam expresses this as `read_after(Option<Position>)` (None = from start). Per-stream `StreamCatchup` keeps its inclusive `Version` semantics internally (it implements `read_after` by `from.map(Version::next)` + inclusive `read_stream`, preserving today's exact behaviour). So the **trait** unifies on "read strictly after," and each impl maps it to its backend.

---

## Task 1: `AllPosition` trait + `RawEventStore` associated type (compile-error-driven)

**Files:** `crates/nexus-store/src/store.rs`

- [ ] **Step 1: Define the trait**

```rust
/// Where an `$all` subscription resumes — adapter-defined. A scalar for an
/// embedded store, a commit-ordered composite for a concurrent SQL store, an
/// LSN for a WAL tail. Carried *alongside* `$all` events (the envelope no
/// longer holds it), and ordered: the loop resumes strictly after the last
/// delivered position via `Ord`, with no successor function.
pub trait AllPosition: Copy + Ord + Send + Sync + core::fmt::Debug + 'static {}
```

- [ ] **Step 2: Add the associated type + retag `read_all` + `AllStream`**

In `RawEventStore`:
```rust
/// The adapter-defined `$all` resume position.
type AllPosition: AllPosition;

/// `$all` stream: position-tagged so resume needs no global field on the
/// envelope. Ordered ascending by `AllPosition`.
type AllStream: futures::Stream<Item = Result<(Self::AllPosition, PersistedEnvelope), Self::Error>>
    + Send + 'static;

/// Open an `$all` read **strictly after** `from` (None = from the beginning).
fn read_all(
    &self,
    from: Option<Self::AllPosition>,
) -> impl std::future::Future<Output = Result<Self::AllStream, Self::Error>> + Send;
```
(`read_all` switches from inclusive `GlobalSeq` to exclusive `Option<AllPosition>` — the locked resume model.)

- [ ] **Step 3: Delete the `GlobalSeq` struct from `store.rs`** (it moves to fjall in Task 6). Expect compile errors in `envelope.rs`, `catchup.rs`, `subscription.rs`, `testing.rs`, fjall — fixed by the following tasks. This is the driving "make it fail, then fix" pass.

---

## Task 2: Drop `global_seq` from the wire frame (V2)

**Files:** `crates/nexus-store/src/wire.rs`

- [ ] **Step 1: Bump the format version + shrink the header.** Add `FrameFormatVersion::V2`; new layout `[u8 ver][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len]…` with `HEADER_FIXED_SIZE = 11` (1+4+2+4). Keep the V1 decode branch (so any local pre-freeze data still reads) — `decode_frame` matches `V1 => decode_frame_v1` (existing) `| V2 => decode_frame_v2`. `encode_frame` emits V2.

- [ ] **Step 2: New `encode_frame` signature** (drop `global_seq`):
```rust
pub fn encode_frame(
    schema_version: SchemaVersion,
    event_type: &EventType,
    payload: &Payload,
    metadata: Option<&Metadata>,
) -> Result<EncodedFrame, WireError>
```
`DecodedFrame` loses its `global_seq` field. `align_padding` recomputes the 16-byte payload boundary from the new `HEADER_FIXED_SIZE` (the version byte still leads; the boundary math is unchanged in form).

- [ ] **Step 3: Update the layout doc comment** (`wire.rs:10-19`) to the V2 layout, and note V1 is retained for transition and removed before freeze.

---

## Task 3: Drop `global_seq` from `PersistedEnvelope`

**Files:** `crates/nexus-store/src/envelope.rs`

- [ ] **Step 1:** Remove the `global_seq: GlobalSeq` field (`:332`), the `global_seq` param from `try_new` (`:360`), the `global_seq()` accessor (`:422`), and the `GlobalSeq::INITIAL` stamp in `for_decode` (`:581`, which now calls the V2 `encode_frame` without it). Update the `try_new` doc + all in-crate constructors/tests that passed a `global_seq`.

- [ ] **Step 2:** Audit every `.global_seq()` call site across the workspace (`git grep '\.global_seq()'`) and remove/replace. The `$all` ones move to reading the tagged position; per-stream ones are deleted (signed off — per-stream events carry no global position).

---

## Task 4: Catchup + live loop thread the tagged position

**Files:** `crates/nexus-store/src/catchup.rs`, `crates/nexus-store/src/subscription_cursor.rs`

- [ ] **Step 1: Reshape the `Catchup` trait** — the scan yields tagged items; resume is exclusive:
```rust
pub trait Catchup: Send {
    type Position: Copy + Send;
    type Scan: futures::Stream<Item = Result<(Self::Position, PersistedEnvelope), Self::Error>> + Send;
    type Error: core::error::Error + Send + Sync + 'static;

    /// Open a bounded scan strictly AFTER `from` (None = from the beginning).
    fn read_after(&self, from: Option<Self::Position>)
        -> impl Future<Output = Result<Self::Scan, Self::Error>> + Send;
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static;
}
```
`position_of` and `next_pos` are **gone** — the position rides on the scan item, and "strictly after" is the adapter's `read_after`.

- [ ] **Step 2: `StreamCatchup`** maps its `Version` path onto the new shape: `read_after(from)` opens `read_stream(id, from.map_or(Version::INITIAL, |v| v.next().unwrap_or(...)))` (preserving today's inclusive+successor behaviour internally) and tags each item `(env.version(), env)`. `Position = Version`.

- [ ] **Step 3: `AllCatchup`** delegates: `Position = S::AllPosition`; `read_after(from)` = `store.read_all(from)` (already yields `(AllPosition, env)`).

- [ ] **Step 4: Rewrite `live`** (`subscription_cursor.rs`) — thread the tagged position:
  - `read_from: Option<C::Position>` stays; the loop opens via `c.read_after(s.read_from)`.
  - On each drained `(pos, env)`: `s.read_from = Some(pos)`; yield `(pos, env)`.
  - Delete `C::next_pos` / `C::position_of` usage. The chunk-reopen + arm-before-confirm-rescan discipline is otherwise **unchanged** (it's the load-bearing lost-wakeup logic — preserve it exactly, only swapping how the position is obtained and that reopen is exclusive).
  - Loop item type becomes `Result<(C::Position, PersistedEnvelope), C::Error>`.

- [ ] **Step 5:** Port the `catchup.rs` + `subscription_cursor.rs` tests to the tagged-item shape (they currently assert `env.global_seq()` / `env.version()` — `$all` ones read the tag; per-stream read the tag or `env.version()`).

---

## Task 5: `Subscription` surface

**Files:** `crates/nexus-store/src/subscription.rs`

- [ ] **Step 1:** `subscribe_all(from: Option<S::AllPosition>)` returns `Stream<Item = Result<(S::AllPosition, PersistedEnvelope), <S as RawEventStore>::Error>>` — the consumer gets the position to checkpoint (it's no longer on the envelope).
- [ ] **Step 2:** `subscribe` (per-stream) — strip the `Version` tag from the loop item and yield bare `Result<PersistedEnvelope, E>` (unchanged consumer API; consumer checkpoints via `env.version()`).
- [ ] **Step 3:** Update the doc comments: `$all` resume is strictly-after the given position; per-stream unchanged.

---

## Task 6: Move `GlobalSeq` into `nexus-fjall`; retag fjall's `$all`

**Files:** `crates/nexus-fjall/src/*`

- [ ] **Step 1:** Define `GlobalSeq(NonZeroU64)` in `nexus-fjall` (move the struct + its `INITIAL`/`new`/`next`/`as_u64` from the old `store.rs`), and `impl nexus_store::AllPosition for GlobalSeq {}`. Set `type AllPosition = GlobalSeq` on `FjallStore`'s `RawEventStore` impl.
- [ ] **Step 2:** `read_all(from: Option<GlobalSeq>)` — the `GlobalScan` cursor opens strictly after `from` (lower bound = `from.map_or(0, |g| g.as_u64()) + 1` keyset on the `events_global` key) and **tags** each item `(GlobalSeq, PersistedEnvelope)` using the **key-derived** `global_seq` (`decode_global_key`). Remove the frame `global_seq` and the key/frame cross-check (`scan.rs:138`) — the frame no longer carries it.
- [ ] **Step 3:** `append` stops passing `global_seq` into `encode_frame` (V2). It still stamps the `events_global` key with the running `global_seq` counter (unchanged) — that's now fjall's private position store.
- [ ] **Step 4:** Update fjall's `wire_key.rs` (`decode_event_value` drops `global_seq`) and all fjall tests asserting `global_seq()` on per-stream reads (those assertions are deleted; `$all` ones read the tag).

---

## Task 7: Verify nexus-store + nexus-fjall

- [ ] **Step 1:** `nix develop -c cargo build --workspace --all-features --all-targets`
- [ ] **Step 2:** `nix develop -c cargo clippy --workspace --all-features --lib -- -D warnings`
- [ ] **Step 3:** `nix develop -c cargo nextest run -p nexus-store -p nexus-fjall --all-features` — the subscription/catchup/wire/envelope suites are the regression surface. Confirm the `$all` catch-up + live-tail tests pass with the tagged-position shape, and the per-stream path is byte-for-byte unchanged.
- [ ] **Step 4:** `nix develop -c cargo fmt --all` + `cargo hakari generate`.
- [ ] **Step 5:** Commit (hook runs `nix flake check`):

```bash
git commit -m "$(cat <<'EOF'
feat(store)!: adapter-defined $all position; drop global_seq from the wire frame (#266)

GlobalSeq did two jobs under one type, both fjall-isms in the shared contract:
a per-event scalar in the frozen frame, and the $all resume position (a scalar
with a +1 successor, derived from the envelope). A concurrent SQL adapter needs
a commit-ordered composite position that fits neither.

Make the $all position adapter-defined: nexus-store owns an AllPosition trait +
RawEventStore::AllPosition; read_all yields position-tagged (AllPosition,
PersistedEnvelope) items and resumes strictly-after (Ord, no successor);
subscribe_all hands the consumer the position to checkpoint. Drop global_seq
from the wire frame (V2) and PersistedEnvelope — read_stream events no longer
carry a global position (it's an $all-only concept). GlobalSeq moves into
nexus-fjall as its position, sourced from the events_global key. Per-stream
Version path unchanged. Prerequisite to the postgres adapter (#213).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Hand off to the postgres adapter

- [ ] **Step 1:** Revise `docs/plans/2026-06-30-postgres-adapter-plan.md`: `type AllPosition = PgAllPos { txid: u64, seq: u64 }` (`Ord` lexicographic); `read_all(from)` = the watermark query with `ORDER BY txid, global_seq` and `WHERE (txid, global_seq) > (from.txid, from.seq)`; the "demonstrate-skip-then-defer" carve-out (old Task 6 + the skip test) is **replaced** by a real "no-skip under concurrent writers" `$all` linearizability test. Remove the position-widening from the out-of-scope list.
- [ ] **Step 2:** Drop the now-redundant `[freeze] Widen the $all cursor position` follow-up that the postgres plan's Task 10.4 anticipated — #266 *is* that work.

---

## Self-Review

**Coverage vs the locked design:** adapter-defined position (Task 1) ✓; position-tagged `$all` + exclusive resume (Tasks 2,4) ✓; `global_seq` out of frame V2 + envelope (Tasks 2,3) ✓; `GlobalSeq` to fjall (Task 6) ✓; consumer gets position via `subscribe_all` (Task 5) ✓; per-stream path untouched (Tasks 4.2, 5.2) ✓; postgres correct-by-construction (Task 8) ✓.

**Placeholder scan:** the trait, `RawEventStore` signatures, `encode_frame` signature, the `Catchup` reshape, and the `live` threading are concrete. Two spots get implementer notes rather than full code because they need a fresh read at execution time: the `Store<S>` delegation impl (mechanical forward of the new assoc types) and the exact `subscribe`/`subscribe_all` return wiring in `subscription.rs` (read the current bodies first).

**Risk:** the `live` loop's lost-wakeup discipline (arm-before-confirm-rescan, chunk reopen) is load-bearing and must be preserved verbatim except for position-source + exclusive-reopen — Task 4.4 calls this out explicitly. The subscription regression suite (Task 7.3) is the guard.

**Ordering:** Task 1 deletes `GlobalSeq` from `store.rs` to drive compile errors through every weld point, so nothing is missed; Tasks 2–6 fix them; the crate first compiles green at the end of Task 6 (atomic, like any public-type move).
