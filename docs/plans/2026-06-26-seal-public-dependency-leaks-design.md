# Seal public dependency leaks before the 1.0 freeze (#208)

**Status:** design approved, pending spec review
**Issue:** #208 (Tier-1 api-freeze blocker)
**Date:** 2026-06-26

## Problem

A public API that names a third-party type couples our SemVer to theirs: a
major bump of that crate becomes *our* breaking change. Before the 1.0 freeze
we must seal every such leak, or accept it deliberately and re-export the type
so downstreams share *our* copy.

## Verified inventory

The freeze-readiness audit (cross-cutting C2) inventoried the leaks. Re-confirmed
against current `main`; four sites drifted since the audit and are marked **(new)**.

| Dep | Leak site | Verdict |
|---|---|---|
| `arrayvec` 0.7 | `nexus::Id::to_label() -> ArrayString<64>` **(new)** | newtype |
| `arrayvec` 0.7 | `nexus::Events::IntoIter` = `Chain<Once, ArrayVecIntoIter>` | seal iterator |
| `arrayvec` 0.7 | `pub use arrayvec::ArrayString;` in `nexus_store` **(new — direct re-export)** | delete |
| `arrayvec` 0.7 | `StoreError`/`AppendError` `stream_id: ArrayString<64>` | newtype |
| `arrayvec` 0.7 | `nexus_store::saga::ProjectedIntents::IntoIter` = `Chain<…, arrayvec::IntoIter>` **(new)** | seal iterator |
| `arrayvec` 0.7 | `FjallError` `stream_id: ArrayString<64>` **and** `reason: ArrayString<128>` **(new — 2 widths)** | newtype |
| `bytes` 1.x | `Encode::encode -> Bytes`; `value`/`envelope`/`cbor` APIs | accept + re-export |
| `futures` 0.3 | `EventStream: futures::Stream`; `subscribe* -> impl futures::Stream` | rebind to `futures_core` + re-export |
| `fjall` 3 | `KeyspaceCreateOptions` in `nexus-fjall` builder closures | accept; independent versioning (#221) |
| `rkyv` 0.8 / `bytemuck` 1 | codec types | feature-gated; document only |
| `thiserror` 2 | derive only | not a leak |

**Already contained (no action):** `BytemuckError(arrayvec::ArrayString<64>)` —
the tuple field is private with no accessor, so the type never escapes.

## Decisions

### 1. `ErrorId` — kernel-owned, const-generic width

A new newtype in the **kernel** (`nexus`), because the kernel itself leaks
arrayvec (`to_label`, `Events::IntoIter`) and so must stop regardless; the store
cannot host a type the kernel needs to return. One type seals kernel + store +
fjall.

```rust
// crates/nexus/src/error_id.rs
pub struct ErrorId<const N: usize = 64>(ArrayString<N>);
```

- `ErrorId<64>` for stream ids; `ErrorId<128>` for fjall's free-text `reason`
  (decision (a): one const-generic type over two named types — a single
  truncation implementation, not one per width).
- Construction from any `Display`: fills the buffer, and **on overflow replaces
  the trailing bytes with `…` (U+2026) at a char boundary** — satisfying
  CLAUDE.md's "truncation must be visually signalled" rule, which no current
  code path honours (`reason_label` and `to_label` both truncate silently).
- Char-boundary safety is a hard requirement: a multi-byte char straddling the
  cap must never panic and must yield valid UTF-8.
- Derives/impls: `Clone, PartialEq, Eq, Hash, Debug`, `Display`, `as_str`,
  `Default` (empty), `From<&str>` is **not** offered (forces the
  truncation-aware constructor). `no_std`-compatible (`core::fmt` + arrayvec).
- `Id::to_label(&self) -> ErrorId` (i.e. `ErrorId<64>`), reimplemented over the
  new constructor. arrayvec stays an internal dependency — just not in any
  public name.

### 2. `bytes` + `futures` re-exports

- **bytes:** `pub use bytes;` in `nexus_store` (kernel doesn't touch bytes).
  Downstreams write `nexus_store::bytes::Bytes` to feed `Encode` / the value
  newtypes. Additive, non-breaking.
- **futures:** rebind the `EventStream` supertrait and the two public
  `subscribe`/`subscribe_all` return types from `futures::Stream` to
  **`futures_core::Stream`**, and `pub use futures_core::Stream`.
  - `futures::Stream` is a re-export of `futures_core::Stream` (identical trait,
    same identity) — so this is **not** an API break; it changes which crate's
    stability the contract is married to. `futures-core` is the small, near-frozen
    definitional crate; `futures` is the churning batteries-included one.
  - `futures` stays a (dev/internal) dependency: `StreamExt`/`TryStreamExt`
    combinators are used internally (repository `try_fold`, subscription loop)
    and by downstreams, but they never appear in our public signatures, so they
    are **not** ours to re-export. Re-export the `Stream` trait only.
  - `futures-core` is already a declared dependency, so the rebind is cheap.

### 3. Seal the iterators — named newtype (forced by stable Rust)

Both `Events::IntoIter` and `ProjectedIntents::IntoIter` get a **named newtype**
wrapping the existing `Chain<Once<_>, ArrayVecIntoIter<_, N>>` and delegating
`next`/`size_hint`. `impl Iterator` in associated-type position
(`type IntoIter = impl Iterator`) requires `#![feature(impl_trait_in_assoc_type)]`,
which is unstable; this workspace pins stable and bans `#![feature]` gates, so the
named newtype is the only option. `ArrayVec`'s `IntoIter` is itself no-alloc, so
wrapping it preserves the kernel's `no_std`/no-alloc story.

```rust
pub struct EventsIntoIter<E: DomainEvent, const N: usize>(
    Chain<Once<E>, ArrayVecIntoIter<E, N>>,
);
// nexus_store::saga::ProjectedIntentsIntoIter<S, N> — analogous.
```

### 4. fjall independent versioning — out of scope here

All crates are `version.workspace = true` (0.1.0). Splitting `nexus-fjall`'s
version so a fjall major majors only the adapter (not kernel/store) is
release-plumbing, overlapping the open **#221** (release-plz fix for the clean
1.0 cut). It touches no leak-sealing code and is folded into that work. The
`KeyspaceCreateOptions` leak needs **no code change** — independent versioning
*is* its mitigation.

## Delivery: 3 code PRs

### PR A — kernel (`nexus`)
- Add `ErrorId<const N = 64>` (`error_id.rs`), re-export `nexus::ErrorId`.
- `Id::to_label -> ErrorId`.
- Seal `Events::IntoIter` via `EventsIntoIter` newtype.
- Self-contained; no downstream dep.

### PR B — store (`nexus-store`) — depends on A
- Adopt `ErrorId` in `StoreError::{Conflict,StreamNotFound}` and
  `AppendError::Conflict` `stream_id` fields; update construction sites.
- **Delete** `pub use arrayvec::ArrayString;`.
- Seal `ProjectedIntents::IntoIter` via `ProjectedIntentsIntoIter` newtype.
- `pub use bytes;`.
- Rebind `EventStream` supertrait + `subscribe`/`subscribe_all` to
  `futures_core::Stream`; `pub use futures_core::Stream`.

### PR C — fjall (`nexus-fjall`) — depends on A, independent of B
- Adopt `ErrorId<64>` for `stream_id` fields; `ErrorId<128>` for `reason`.
- `reason_label -> ErrorId<128>`; update construction sites (most flow through
  `id.to_label()` and update for free; the `ArrayString::new()` empty-label sites
  in `scan.rs` become `ErrorId::default()`).

### Deferred
- fjall independent versioning → #221.

## Breaking-change ledger (all intentional, pre-freeze)
- `nexus_store::ArrayString` re-export removed.
- Error `stream_id`/`reason` field types change `ArrayString<N>` → `ErrorId<N>`.
- `Events::IntoIter` / `ProjectedIntents::IntoIter` associated types renamed.
- `Id::to_label` return type changes.
- Additive (non-breaking): `pub use bytes`, `pub use futures_core::Stream`.
- Non-breaking: `EventStream`/`subscribe` rebind (same trait identity).

## Testing (CLAUDE.md rule 7 — 4 categories first, on `ErrorId`)
1. **Sequence/protocol:** construct → truncate → `as_str`/`Display` → compare/hash.
2. **Lifecycle:** round-trip an `ErrorId` through an error enum's `Display`/`Debug`.
3. **Defensive boundary:** input length ∈ {0, 1, N-1, N, N+1}; multi-byte UTF-8
   char straddling the cap (no panic, valid UTF-8, ends in `…`); empty input.
4. **Linearizability/isolation:** n/a (`ErrorId` is immutable, no shared state).
- Property test (boundaries included): for any input, the result is valid UTF-8,
  `len() <= N`, and is `…`-suffixed iff the input did not fit.
