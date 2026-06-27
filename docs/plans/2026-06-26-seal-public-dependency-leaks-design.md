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

## Delivery: 4 code PRs (workspace stays green at every step)

**Sequencing constraint:** `Id::to_label() -> ArrayString<64>` is consumed by
`nexus-store` and `nexus-fjall` (fjall assigns `id.to_label()` to ~14
`ArrayString<64>` error fields). The gate runs `nix flake check` over the whole
workspace, so retyping `to_label` and the downstream fields must not be split
across PRs that each have to compile. The arrayvec seal is therefore staged so
`to_label`'s retype lands **last**, after every consumer has migrated to
constructing `ErrorId` directly via `ErrorId::from_display`.

### PR 1 — kernel additive (`nexus`) — no downstream dependency
- Add `ErrorId<const N = 64>` (`error_id.rs`), re-export `nexus::ErrorId`.
- Seal `Events::IntoIter` via the `EventsIntoIter` newtype.
- **`to_label` is left unchanged** (`ArrayString<64>`) — retyped in PR 4.
- Purely additive; workspace compiles unchanged.

### PR 2 — store (`nexus-store`) — depends on PR 1
- Adopt `ErrorId` in `StoreError::{Conflict,StreamNotFound}` `stream_id` fields
  **only**; construct via `ErrorId::from_display(&id)` (not `to_label`).
  - **Scope correction (vs. the inventory):** `AppendError::Conflict`'s
    `stream_id` is **not** retyped here. `AppendError::Conflict` is *constructed
    by `nexus-fjall`* (`crates/nexus-fjall/src/store.rs`), so retyping its field
    can't land in a store-only PR without breaking the fjall build — the same
    cross-crate binding that defers `to_label` to PR 4. It moves to **PR 3**
    (atomic with fjall). The store-side facade arms that map
    `AppendError::Conflict → StoreError::Conflict` (`repository.rs`) therefore
    do a transitional `ErrorId::from_display(&stream_id)` conversion, removed in
    PR 3 once `AppendError` is also `ErrorId`.
- **Delete** `pub use arrayvec::ArrayString;`.
- Seal `ProjectedIntents::IntoIter` via `ProjectedIntentsIntoIter` newtype.
- `pub use bytes;`.
- Rebind `EventStream` supertrait + `subscribe`/`subscribe_all` to
  `futures_core::Stream`; `pub use futures_core::Stream`.

### PR 3 — fjall (`nexus-fjall`) — depends on PR 1, independent of PR 2
- Adopt `ErrorId<64>` for `stream_id` fields; `ErrorId<128>` for `reason`.
- **Also retype `AppendError::Conflict`'s `stream_id`** to `ErrorId` (deferred
  from PR 2): atomic with the fjall construction sites that build it, and drop
  the transitional `ErrorId::from_display` conversion in the store facade arms.
- Replace the `id.to_label()` sites and `reason_label`/`ArrayString::new()`
  sites with `ErrorId::from_display(&id)` / `ErrorId::default()`.

### PR 4 — kernel, seal `to_label` (`nexus`) — depends on PR 2 + PR 3
- Retype `Id::to_label -> ErrorId`. No consumer depends on the old
  `ArrayString<64>` return by now, so this compiles workspace-wide.
- Update any remaining in-kernel / example callers.

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
