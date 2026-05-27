# Bytes Envelope Refactor — Design

**Decision date:** 2026-05-27
**Companion:** [`2026-05-27-bytes-envelope-pr1-implementation.md`](./2026-05-27-bytes-envelope-pr1-implementation.md), [`2026-05-27-bytes-envelope-deviations.md`](./2026-05-27-bytes-envelope-deviations.md)

## Goal

Three intertwined changes, sequenced as 3 PRs:

1. Replace the `M = ()` metadata generic on every persistence trait with `Option<Bytes>` at the envelope level.
2. Migrate envelopes (`PendingEnvelope`, `PersistedEnvelope`) from lifetime-borrowed `&[u8]` to owned `bytes::Bytes` using the **"single Bytes + Range<u32> offsets"** pattern.
3. Collapse `EventStream` / `BaseEventStream` / `EventStreamExt` / `OwnedEventStream` / `IntoStream` and all combinator types (`Map`, `TryMap`, `MapErr`, `TryScan`) into a single marker trait over `futures::Stream<Item = Result<PersistedEnvelope, E>> + Send`. Combinators come from `futures::StreamExt`.

This is a pre-1.0 breaking change. Per project [main merge policy](../../CLAUDE.md), squash via PR.

## Why now

Three triggers converged:

**Postgres adapter realism.** sqlx's `BYTEA` decode has no `Bytes` impl and the row buffer's internal `Bytes` field is `pub(crate)` (`sqlx-postgres/src/value.rs:19`). A borrowed-row `PersistedEnvelope<'a>` cannot serve a sqlx-style driver cleanly. Once postgres is on the roadmap, GAT lending streams structurally fail.

**`M` is threaded but never active.** Audit (2026-05-27) found `M` carried by 10+ traits but hardcoded to `()` at every adapter (`nexus-fjall/src/store.rs:60`, `nexus-store/src/testing.rs:145`). No production code paths round-trip non-`()` metadata. Zero callsites in `examples/` or `tests/`. Per [CLAUDE.md §4](../../CLAUDE.md), unused generics get removed.

**Stream extraction project dies.** The earlier direction (extract `stream/` as standalone combinator crate) was built on the premise that we'd ship our own lending-GAT combinators. Collapsing to `futures::Stream` means the "library" is `futures::Stream` + `futures::StreamExt`, maintained by tokio-rs. We ship nothing.

## Options considered

| | Per-yield atomic ops | Envelope size | Industry precedent | Postgres-ready | `futures::Stream` interop |
|---|---|---|---|---|---|
| A. Keep GAT, make adapters persist `M` | 2 | ~48 bytes | rare | no | bridge code |
| B. prost-style independent `Bytes` per field | ~6 (N=3) | 96 bytes | universal | yes | free |
| **C. Single `Bytes` + `Range<u32>` offsets** | **2** | **56 bytes** | uncommon (private in redis-protocol) | yes | free |

**Chose C.** Matches GAT's per-yield cost. Enables postgres + futures ecosystem. ~40% smaller envelopes than B. The "no public-API precedent" cost is real but bounded — `redis-protocol`'s `RangeFrame` is the same shape, kept private only because their parser produces it transiently. Our case is whole-row decode, which fits ranges structurally.

## Source-verified facts

From `bytes/src/bytes.rs` (commit `2256e6dc`):

- `Bytes::slice(range)` = exactly one `fetch_add(1, Relaxed)` on root Arc (line 373-386 → 1441-1454). Zero memcpy.
- `size_of::<Bytes>() == 32` on 64-bit. `size_of::<Option<Bytes>>() == 32` (niche via non-null vtable pointer).
- `Bytes::is_unique()` exists but does not skip the atomic op on `slice()`.

From `fjall/Cargo.toml` and `lsm-tree/src/slice/slice_bytes/mod.rs:91-101`:

- fjall has a `bytes_1` feature that flips `Slice` to be a newtype over `bytes::Bytes` with zero-copy `From<Bytes>` / `From<Slice> for Bytes`. **Load-bearing for this refactor** — without it, every fjall read pays 1 alloc + memcpy to convert.

From `sqlx-postgres/src/types/bytes.rs:41-106`:

- Decode impls: `&'r [u8]`, `Vec<u8>`, `[u8; N]`. No `Bytes` impl exists. `Vec<u8>` decode always allocates + memcpys.
- Encode impls: `&[u8]`, `Vec<u8>`, `[u8; N]`, `Cow<'_, [u8]>`. Always copies into `PgArgumentBuffer`.

## The two footguns

Both came back from the source read and must be designed around in implementation:

### F1. Empty-range collapse

`bytes/src/bytes.rs:192-205, 377` — `Bytes::slice(begin..begin)` short-circuits to `STATIC_VTABLE` with a synthesized pointer, which does NOT hold the parent buffer alive.

**Fix:** Use `Option<Range<u32>>` for metadata where `None` = absent. Never construct `Some(empty)`. The encoding sentinel `meta_len == u32::MAX` maps to `None`; `meta_len == 0` is treated as `Some(_)` with an empty range only if "present but empty" semantics are needed (currently not — treat as `None`).

### F2. No compile-time link between Range and Bytes

If the backing `Bytes` were swapped, ranges silently index wrong data.

**Fix:** `PersistedEnvelope` fields stay private. Construction only via `try_new()` which validates `range.end <= value.len()`. No setter for the backing `value: Bytes`. Immutable after construction. The footgun is constrained to the cursor code in fjall/InMemoryStore — small, testable surface.

## Wire format change

Current event value (`nexus-fjall/src/encoding.rs:137`):

```
[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]
```

New event value:

```
[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len][event_type UTF-8][meta][payload]
```

- Header size grows from 14 to 18 bytes (`EVENT_VALUE_HEADER_SIZE`).
- `meta_len == u32::MAX` is the absent sentinel (distinguishing from `meta_len == 0`).
- Per [project_main_merge_policy](#) this is a breaking on-disk format change. Pre-1.0 acceptable; no migration tooling shipped. Existing fjall stores from before this PR cannot be read.

## Trait collapse

**Before** (current):
```
RawEventStore<M>
  ├─ EventStream<M> (GAT type Item<'a>)
  │   └─ BaseEventStream<M> (HRTB-routing sub-trait)
  │       └─ EventStreamExt<M> (combinators: try_fold, etc.)
  └─ OwnedEventStream<M> (on-ramp to futures::Stream)
      ├─ Map<S, F>
      ├─ TryMap<S, F, E>
      ├─ MapErr<S, F>
      ├─ TryScan<...>
      └─ IntoStream<S, M>
Subscription<M> / SubscriptionBackend<M> / SharedSubscription / SharedSubscriptionBackend<M>
```

**After**:
```
RawEventStore
  └─ EventStream: futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send
Subscription / SubscriptionBackend / SharedSubscription / SharedSubscriptionBackend
```

Deleted: `BaseEventStream`, `EventStreamExt`, `OwnedEventStream`, `IntoStream`, `Map`, `TryMap`, `MapErr`, `TryScan`. All combinators come from `futures::StreamExt`.

## PR sequence

1. **PR 1** — Envelope shape + wire format + adapters. Bytes-ranges envelopes, new encoding, fjall + InMemoryStore + repository facade updated. Stream traits still carry `M` (collapsed to `()` at adapter sites); next PR removes them.
2. **PR 2** — Drop `M` generic from every trait; collapse stream trait family to single `EventStream: futures::Stream + Send` marker; delete combinator types; update projection runner to use `futures::StreamExt`.
3. **PR 3** — Docs (README, module-level rustdoc, CLAUDE.md architecture section); example updates to use `futures::StreamExt` combinators.

## Non-goals

- No outbox/queue/transactional-publish framework (deferred until concrete consumer).
- No postgres adapter in this refactor (this enables it; the adapter is later work).
- No typed-metadata facade codec (YAGNI — bytes at storage; typed only when a real consumer needs it).
- No on-disk migration tooling (pre-1.0 acceptable).
- No re-introduction of borrowed cursor paths after collapse — owned `futures::Stream` is the single shape.
