# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nexus is an event-sourcing and DDD kernel for Rust. It provides composable traits, derive macros, and infrastructure adapters (starting with fjall). The project is **experimental** with an unstable API.

## Build & Development Commands

**Prerequisites:** Nix with flakes enabled. Use `nix develop` (or `direnv allow`) to enter the dev shell.

```bash
# Run ALL checks (clippy, fmt, tests, taplo, audit, deny, hakari)
nix flake check

# Run a single test by name
cargo test -p nexus-store -- envelope_tests

# Apply formatting
cargo fmt --all
```

## Architecture

### Crate Dependency Graph

```
nexus-framework --> nexus-store --> nexus (kernel)
nexus-fjall   --> nexus-store
nexus-macros <-- nexus (kernel, optional via "derive" feature)
```

### Kernel Crate (`nexus`) — Flat Module Layout

- **`aggregate.rs`** — `Aggregate` trait (binds State + Error + Id), `AggregateRoot<A>` (read-only state container with version tracking + `replay` + `advance_version` + `apply_events`), `AggregateState` trait (`initial()`, `apply(self, &Event) -> Self`; requires `Clone` for panic safety), `AggregateEntity` trait (newtype delegation pattern with default methods: `id`, `state`, `version`, `replay`), `Handle<C, const N: usize = 0>` trait (per-command decide function returning `Events<E, N>`; N declares max additional events, default 0 = single event). Configurable limit: `MAX_REHYDRATION_EVENTS` (default 1M, `NonZeroUsize`).
- **`version.rs`** — `Version` newtype over `NonZeroU64` (event versions always >= 1). `Version::new(u64) -> Option<Self>` (mirrors `NonZeroU64::new`), `Version::INITIAL` = 1, `Version::next() -> Option<Self>`. `VersionedEvent<E>` pairs an event with its version.
- **`event.rs`** / **`events.rs`** — `DomainEvent` trait (extends `Message`, provides `name() -> &'static str`). `Events<E, const N: usize = 0>` is an `ArrayVec`-backed collection guaranteeing at least one event with compile-time capacity N+1; constructed via `events![e1, e2]` macro. N=0 (default) = single event, no heap allocation, `no_std`/`no_alloc` compatible.
- **`error.rs`** — `KernelError`: `VersionMismatch { expected, actual }`, `RehydrationLimitExceeded { max }`, `VersionOverflow`.
- **`id.rs`** — `Id` marker trait requiring `Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static`. `AsRef<[u8]>` provides stable byte representation for storage keys (not `Display`, which is for humans). Default method `to_label() -> ArrayString<64>` provides allocation-free diagnostic labels for error messages.
- **`message.rs`** — `Message` base marker trait (`Send + Sync + Debug + 'static`).

### Store Crate (`nexus-store`) — Persistence Edge Layer

Flat layout — one file per concept, no module subdirectories. The earlier `codec/`, `envelope/`, `upcasting/`, `store/`, `repository/`, `state/`, `projection/` directories were collapsed into single files because each held only a handful of small files with no cohesion benefit. The boundary that matters is the crate boundary (kernel-pure → store-persistence → adapters); the boundary that didn't matter was inside `nexus-store`.

- **`codec.rs`** — Serialization traits. One `Encode<E>` + one `Decode<E>` (two traits, not three — the old `BorrowingDecode` was a workaround for the borrowed-cursor lifetime cliff, which the owned-`Bytes` envelope eliminated).
  - `Encode<E>` returns `bytes::Bytes` so the encoded payload flows end-to-end with no `Vec → Bytes` adapter step.
  - `Decode<E>` has a `type Output<'a>` GAT: `E` for owning codecs (serde), `&'a Archived<E>` for rkyv, `&'a E` for bytemuck POD. `decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<Self::Output<'a>, _>` — the codec reaches into the envelope for `event_type()` and `payload()` itself.
  - `E: ?Sized` so unsized archived types (`Archived<MyEvent>`, `[u8]`, `str`) are representable.
  - Inline `pub mod serde` block (feature-gated): `SerdeFormat`, `SerdeCodec<F>`, nested `pub mod json` with `Json` + `JsonCodec` alias.
  - Inline `pub mod bytemuck` block (feature-gated: `bytemuck`): `BytemuckCodec` for `#[repr(C)]` POD types. `BytemuckError` wraps upstream `PodCastError` because the upstream type doesn't implement `std::error::Error`.
  - Inline `pub mod rkyv` block (feature-gated: `rkyv`): `RkyvCodec` for rkyv-archived types via `to_bytes_in(value, AlignedVec::new())` + `access::<E::Archived, rancor::Error>`.
- **`envelope.rs`** — Event containers for read & write paths.
  - `PendingEnvelope` (owned, write-path) built via typestate builder: `pending_envelope(version).event_type().payload().build(metadata)`. No `M` metadata generic — metadata is plain `Option<Bytes>` on the envelope.
  - `PersistedEnvelope` (owned, read-path) — single owned `bytes::Bytes` buffer + cached `Range<u32>` offsets for `event_type` / `metadata` / `payload`. Cheap-to-clone (one Arc inc + range copies), no lifetime parameter, so it flows through `futures::Stream` items without bridging code. Constructed only via `try_new` which validates `range.end <= value.len()`.
  - `PersistedEnvelope::for_decode(event_type, payload)` synthesizes an envelope from raw `(name, payload)` for upcaster post-transform and snapshot-bytes paths that lack a real envelope.
- **`upcasting.rs`** — Schema evolution.
  - `EventMorsel<'a>`: zero-copy-when-possible data unit flowing through transforms.
  - `Upcaster` trait for raw-byte schema migrations with associated `type Error`. `()` is the no-op passthrough (`Error = Infallible`).
- **`store.rs`** — Storage infrastructure traits that adapters (e.g. `nexus-fjall`) implement. No `M = ()` metadata generic on any of these — the audit (2026-05-27) found it threaded through 10+ traits but hardcoded to `()` everywhere, so it was removed.
  - `Store<S>`: `Arc`-wrapped shared handle to any `RawEventStore`. Clone-cheap; carries no codec/upcaster/aggregate binding. Call `.repository()` to obtain a `RepositoryBuilder`. `.raw()` is the substrate escape hatch for users who need direct `RawEventStore` access; `pub(crate) .arc()` exposes the inner `Arc<S>` to the subscription module.
  - `RawEventStore` trait: byte-level `append` and `read_stream`. `append` stamps each event with the next `GlobalSeq`. `read_stream` returns the adapter's concrete owned `Stream` associated type.
  - `GlobalSeq`: store-local global sequence number stamped on every event at append time — the `Version` analogue across *all* of one producer's streams, and the position an all-streams subscription resumes from. **Monotonic but not gapless** — an aborted append may burn values, so consumers must tolerate gaps.
- **`subscription.rs`** — Subscription primitive split across two shapes.
  - `Subscription<S>`: user-facing concrete struct. Built via `Subscription::new(&store)` from a `Store<S>` handle (clones the inner `Arc` once); `.subscribe(&id, from)` returns a `futures::Stream` cursor that **never returns `None`** — when caught up, it waits for new events instead of terminating. `from: None` = beginning; `from: Some(v)` = events *after* `v`. Users never touch `Arc`.
  - `RawSubscription`: adapter-facing trait. Adapters `impl RawSubscription for FjallStore` (or `InMemoryStore`) on the bare store type, not on `Arc<Self>` — the orphan rule otherwise forbids `impl Subscription for Arc<FjallStore>` in an external crate, so the indirection through a trait whose `Self` is the bare store type is mandatory. Sealed via `pub mod sealed { pub trait Sealed {} }`. The `Sealed` super-trait is `pub` (so external adapter crates can implement it), but neither `Sealed` nor `RawSubscription` is re-exported at the crate root. The signal that this is adapter-facing is documentary, not structural — users grep `Subscription` and find the user-facing struct; adapter authors import the qualified path `nexus_store::subscription::RawSubscription`.
- **`stream.rs`** — `EventStream` marker trait over `futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send`. No methods of its own. The associated `Error` type is recovered from the stream's `Item` so call sites can bound on `S: EventStream<Error = MyErr>` instead of carrying an extra generic. All combinators come from `futures::StreamExt` / `TryStreamExt`. Replaced the GAT-lending `EventStream` + `BaseEventStream` + `EventStreamExt` + `OwnedEventStream` + `IntoStream` + `Map` + `TryMap` + `MapErr` + `TryScan` (~1370 lines deleted) — the owned `Bytes` envelope removed the lifetime cliff that motivated the GAT.
- **`wire.rs`** — One canonical row builder used by every adapter: `wire::build_row(global_seq, schema_version, event_type, metadata, payload) -> Result<RowBuilt, WireError>` and `wire::decode_row(&[u8]) -> Result<DecodedRow, DecodeError>`. Layout: `[u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len][event_type][metadata?][padding][payload]` where `meta_len == u32::MAX` is the absent-metadata sentinel. The padding makes the payload pointer land on a **16-byte boundary** inside the returned `Bytes` — a wire-format invariant zero-copy decoders (rkyv default alignment, flatbuffers, `#[repr(C)]` POD up to 16) rely on. Backing buffer built via `aligned_vec::AVec<u8, ConstAlign<16>>` → `bytes::Bytes::from_owner(avec)` for zero-copy ownership transfer that preserves alignment.
- **`state.rs`** — Unified snapshot persistence. One trait powers both projection state and aggregate snapshots.
  - `SnapshotStore<S, P>` trait: atomic persistence of derived state plus the position it was folded to. `hydrate(id, schema_version) -> Option<(P, S)>` and `commit(id, schema_version, position, &state)` — state and position are saved and loaded *together*, never separately, so a half-write is unrepresentable. Generic over the position type `P` (`Version` for a single stream, `GlobalSeq` for a multi-stream projection).
  - `CodecSnapshotStore<SS, C>`: bridges a byte-level `SnapshotStore<Vec<u8>, P>` (what fjall implements) to a typed `SnapshotStore<S, P>` via `Decode<S>` / `Encode<S>`. The position `P` passes through untouched. `CodecSnapshotStoreError::Wire` covers the rare wire-build failure when synthesizing an envelope from snapshot bytes.
  - `PersistTrigger` trait: `EveryNEvents(N)` (bucket-crossing), `AfterEventTypes(&[&str])` (semantic). Used by both projection runners and the snapshot decorator.
  - Inline `#[cfg(feature = "testing")] mod testing` block: `InMemorySnapshotStore`.
- **`projection.rs`** — Feature-gated under `projection`. Slim by design.
  - `Projector` trait: pure fallible fold function (`initial()` + `apply(state, &event) -> Result`). Fallibility is intentional: projections may do checked arithmetic where aggregates do not. Recovery policy (skip/fail/dead-letter) is the framework's concern, not the projector's. The IO-driven runner lives in `nexus-framework`.
- **`repository.rs`** — Aggregate-facing trait + its two facade impls.
  - `Repository<A>` trait: high-level `load` and `save` for aggregates.
  - `EventStore<S, C, U>` facade — owning codec; HRTB bound `for<'a> C: Encode<E> + Decode<E, Output<'a> = E> + 'static`.
  - `ZeroCopyEventStore<S, C, U>` facade — borrowing codec; HRTB bound `for<'a> C: Encode<E> + Decode<E, Output<'a> = &'a E> + 'static`. Combined under one HRTB to satisfy `clippy::type_repetition_in_bounds` without weakening the constraint.
  - Both facades drive event-stream consumption through `futures::TryStreamExt::try_fold`. Closures use `Arc`-wrapped codec/upcaster captures so `FnMut` can move them.
  - `pub(crate) ReplayFrom<A>` trait shared with the `Snapshotting` decorator.
- **`builder.rs`** — `RepositoryBuilder` typestate builder with `!Send` marker `NeedsCodec`; produces `EventStore` or `ZeroCopyEventStore`. `NoSnapshot` is the default snapshot slot; `WithSnapshot<SS, T>` is the active slot (feature-gated: `snapshot`). Kept separate from `repository.rs` because it's pure typestate construction scaffolding (~390 lines).
- **`snapshot.rs`** — `Snapshotting<R, SS, T>` decorator (feature-gated: `snapshot`). Wraps an inner repository and transparently hydrates from a `SnapshotStore<A::State, Version>` on read, commits on write per a `PersistTrigger`. Snapshot save is best-effort (never blocks event persistence). Kept separate from `repository.rs` because it's the only feature-gated piece of the facade.
- **`error.rs`** — `StoreError<A, C, U>` (generic over adapter/codec/upcaster errors, allocation-free), `AppendError<E>`, `UpcastError<U>` (generic over transform error), `InvalidSchemaVersion`. All use `ArrayString<64>` for stream IDs instead of heap-allocated strings.
- **`testing.rs`** — `InMemoryStore`, `InMemoryStream`, `InMemoryStoreError` (feature-gated behind `testing`).

Feature flags: `serde`, `json` (implies `serde`), `snapshot`, `snapshot-json`, `projection`, `projection-json`, `testing`.

Public sub-paths after the flatten: `nexus_store::codec::*`, `::envelope::*`, `::upcasting::*`, `::store::*`, `::state::*`, `::projection::*`, `::repository::*` (Repository + facades), `::builder::*` (builder typestate), `::snapshot::*` (decorator). All top-level types remain re-exported at `nexus_store::*` for convenience.

### Framework Crate (`nexus-framework`) — Batteries-Included Runtime Layer

Depends on `nexus-store` (with `projection` feature) + `tokio` + `futures`. Provides IO-driven components that require an async runtime. The kernel and store remain runtime-agnostic; tokio is confined here.

- **`projection/`** — Subscription-powered CQRS projection runner. Built on `Subscription<S>` + `SnapshotStore` from `nexus-store`.
  - `projection.rs` — `Projection<I, Sub, SS, P, EC, Trig, Mode>`: two-phase typestate. `Configured` (built, not loaded) → `initialize()` → `Ready<S>` (loaded, can run). `initialize()` does one `hydrate()` and resolves to `Fresh` (nothing persisted) or `Resume` (snapshot loaded). `Ready` carries `ProjectionStatus<S>` and `StartupDecision` (Fresh/Resume/Rebuild — the label is for supervisor inspection only). `run()` subscribes and enters the event loop — a `tokio::pin!` + `tokio::select!` between `futures::Stream::next` (the subscription) and a `shutdown` future. (Replaces an earlier hand-rolled `try_fold_async_until` combinator on a GAT lending stream trait; the 2026-05-27 collapse of event streams to plain `futures::Stream` made the custom combinator unnecessary.) `rebuild()` resets to initial state and is the only path that produces `StartupDecision::Rebuild`.
  - `status.rs` — `ProjectionStatus<S>` enum: explicit FSM for the event loop with three write-centric states (`Idle`, `Pending`, `Committed`). Pure sync `apply_event` transition function — no IO, no async. Driven by the async shell in `Projection::run()`.
  - `builder.rs` — `ProjectionBuilder`: typestate builder with `!Send` marker structs (`NeedsSub`, `NeedsSnap`, `NeedsProj`, `NeedsEvtCodec`) that prevent calling `.build()` until each required field is set. The snapshot store is required; trigger defaults to `EveryNEvents(1)` (persist every event).
  - `error.rs` — `ProjectionError<P, EC, SS, Sub>`: one variant per failure domain (projector, event codec, snapshot store, subscription).

The framework persists projection state through `nexus-store::state::SnapshotStore<P::State, Version>` — the same trait the aggregate snapshot decorator uses. State and resume position are committed together, atomically; there is no separate checkpoint store and no checkpoint-only mode.

### Fjall Adapter Crate (`nexus-fjall`) — Embedded LSM-Tree Event Store

The workspace pins `fjall = { version = "3", features = ["bytes_1"] }`. That feature is **load-bearing**: it flips fjall's `Slice` value to be a newtype over `bytes::Bytes` with zero-copy `From<Bytes>` / `From<Slice> for Bytes`. Without it every fjall read on the hot path would pay one alloc + memcpy to convert. With it, the `bytes::Bytes` inside `PersistedEnvelope` *is* the same Arc-counted buffer fjall handed us.

- **`store.rs`** — `FjallStore` implements `RawEventStore`, `RawSubscription` (via `subscription_stream`), and — under the `snapshot` feature — `SnapshotStore<Vec<u8>, Version>`. Partitions: `streams` (id bytes → version counter, point-read optimized), `events` (event rows, scan-optimized, LZ4 compressed), `global` (one key holding the store-wide `GlobalSeq` counter), and `snapshots` (under the `snapshot` feature). All writes go through atomic fjall transactions; `append` claims its `GlobalSeq` range inside the same transaction as the event writes. `append` builds every row via `nexus_store::wire::build_row` so the on-disk payload bytes land on a 16-byte boundary.
- **`builder.rs`** — `FjallStoreBuilder`: `FjallStore::builder(path).streams_config(fn).events_config(fn).open()`.
- **`partition.rs`** — Sealed `KeyspaceConfig` trait. Only `()` (defaults) and `FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions` closures implement it. Eliminates dynamic dispatch from the builder by monomorphising keyspace configuration at compile time.
- **`stream.rs`** — `FjallStream` implements `futures::Stream<Item = Result<PersistedEnvelope, FjallError>>` as a concrete async cursor over eagerly-loaded rows. No GAT, no bespoke combinator trait — consumers get the full `futures::StreamExt` / `TryStreamExt` combinator surface for free.
- **`subscription_stream.rs`** — `FjallSubscriptionStream` cursor built with `futures::stream::unfold` for the notify/refill loop; this is the `Stream` associated type the `RawSubscription` impl returns. Re-reads the underlying store when caught up rather than terminating. `OwnedStreamId` captures stream bytes to satisfy the `Id` `'static` bound across re-reads.
- **`encoding.rs`** — Binary key encoding only. Event keys: `[u16 BE id_len][id_bytes][u64 BE version]` (length-prefixed to prevent prefix collisions in range scans). The value layout is owned entirely by `nexus_store::wire` — `encoding::encode_event_value` / `decode_event_value` are thin wrappers around `wire::build_row` / `wire::decode_row` that preserve the adapter-local test/bench API surface.
- **`error.rs`** — `FjallError`: `Io`, `CorruptValue`, `CorruptMeta`, `InvalidInput`, `VersionOverflow`, `GlobalSeqOverflow`. All diagnostic fields use `ArrayString<64>` / `ArrayString<128>` — no heap allocation on error paths.

### Aggregate Lifecycle (Key Flow)

1. Define aggregate via `#[nexus::aggregate(state = S, error = E, id = I)]` on a unit struct, or implement `Aggregate` + `AggregateEntity` + `Handle<C, N>` manually
2. `Aggregate::new(id)` creates a fresh aggregate (delegates to `AggregateRoot::new`; version = `None`)
3. **Load (rehydration):** `root.replay(version, &event)` replays persisted events with strict version validation (must start at 1, strictly sequential, enforces `MAX_REHYDRATION_EVENTS`)
4. **Decide:** `aggregate.handle(command) -> Result<Events<E, N>, Error>` — pure decision function, reads state via `&self`, returns decided events without mutating. N is the const generic capacity declared on `Handle<C, N>` (default 0 = single event).
5. **Persist + advance:** Repository writes events to store, then calls `root.advance_version(new_version)` + `root.apply_events(&events)` to sync in-memory state

### `nexus-macros` — Proc Macros

Three macros:
- **`#[nexus::aggregate(state = S, error = E, id = I)]`** — Attribute macro on a unit struct. Generates `Aggregate` impl, `AggregateEntity` impl, newtype wrapping `AggregateRoot`, `new(id)` constructor, and redacted `Debug` (shows only id + version).
- **`#[derive(DomainEvent)]`** — Derive macro for enums only. Generates `Message` + `DomainEvent` impls, with `name()` returning variant names as `&'static str`.
- **`#[nexus::transforms(aggregate = A, error = E)]`** — Attribute macro on an impl block. Generates `Upcaster` impl with `type Error = E`, iterative apply loop, and `current_version()` lookup. Transform functions annotated with `#[transform(event = "...", from = N, to = N+1)]`.

### Examples

- **`inmemory`** — Pure in-memory event sourcing, no persistence (bank account domain)
- **`store-inmemory`** — Demonstrates all `nexus-store` traits with `InMemoryStore`, including codec, upcasting, schema evolution, and the "substrate path" (Step 7) — driving `Store::raw()` directly with `futures::StreamExt::map_err` + `futures::TryStreamExt::try_fold` instead of going through the typed repository facade
- **`store-and-kernel`** — Full lifecycle integrating kernel + store: create → decide → encode → persist → read → decode → rehydrate

## Mandatory Rules

These rules are non-negotiable. Every one exists because of a real bug found in this codebase.

### 0. No Assumptions, No Opinions — Facts Only

- **Never assume.** If you don't know something, say so and research it. Do not fill gaps with plausible-sounding guesses about APIs, crate behavior, algorithm properties, or performance characteristics.
- **Never give opinions.** No "I think," "it would be cleaner," "this feels better," "my preference is." Claims must be grounded in verifiable evidence.
- **Facts must cite sources.** Every technical claim about algorithms, data structures, concurrency, cryptography, or systems behavior must be backed by:
  - **arXiv papers** (preferred for algorithmic/theoretical claims)
  - **Peer-reviewed research papers** (conferences: SOSP, OSDI, VLDB, SIGMOD, PLDI, POPL, CRYPTO, etc.)
  - **Primary source GitHub repositories** (the actual implementation being discussed, not blog posts about it)
  - **Official specifications / RFCs** for protocols and standards
- **"Common knowledge" is not a source.** If a claim is worth making, it is worth citing. If no source exists, do not make the claim.
- **Uncertainty is a fact too.** When evidence is incomplete or contradictory, state the uncertainty explicitly rather than collapsing it into a confident-sounding opinion.

### 1. Database Atomicity

Every database interaction in store adapters MUST be atomic:
- **Reads touching multiple partitions/keys**: single read transaction or shared snapshot — NEVER two independent reads
- **Writes**: write transactions
- **Read-then-write**: single transaction spanning both
- **Inline projections**: projection state writes default to best-effort (separate operation, like snapshots). Same-transaction atomicity is an opt-in capability for adapters that support it — not a universal requirement. Projections are derived state; if a best-effort write fails, the projection is re-derived on next load or rebuild.

If a public method does 2+ database calls without a shared transaction, it is a bug.

### 2. Arithmetic Safety

- **No bare arithmetic** in production code. Use `checked_add`/`checked_sub` and return `Err` on overflow. `saturating_add` is banned — it silently stops progress and converts overflow into misleading downstream errors.
- **No `unwrap_or(sentinel)`** for failed conversions. `u64::try_from(x).unwrap_or(u64::MAX)` hides the root cause. Return a proper error.
- **`debug_assert` is NOT a safety check.** If an invariant violation would corrupt data or produce silently wrong results, use a runtime check (`return Err(...)` or `assert!`). `debug_assert` is compiled out in release — it protects nothing in production. Reserve it only for conditions that are provably impossible by construction (e.g., postconditions of code you just ran).

### 3. Error Handling

- **One variant = one failure domain.** Never jam unrelated errors into an existing variant. If upcast fails, add `StoreError::Upcast`, don't reuse `StoreError::Codec`.
- **Never erase structured errors into `Box<dyn Error>`** when callers need to match on them. Use typed enum variants with `#[source]`.
- **Never discard the original error** with `|_|` in `map_err`. Wrap it via `#[source]`/`#[from]`, or at minimum preserve its message.
- **Never box string literals as errors.** `"some message".into()` as `Box<dyn Error>` has no type name, no fields, no downcast target.
- **Unknown values must be `Option`, not sentinels.** When a version is unknowable (e.g., corrupt key), use `Option<u64>`, not `version: 0`.
- **Overflow/limit errors are not concurrency conflicts.** Never reuse a retry-eligible error code (`Conflict`) for a non-retryable condition (arithmetic overflow).
- **Write-path and read-path must enforce the same invariants the same way.** If the read path rejects `schema_version == 0`, the write path must too — not silently clamp it to 1.
- **Never use `#[non_exhaustive]` on enums.** During experimental/unstable API development, it adds friction (forces `_ =>` wildcard arms) without value — breaking changes are expected. Use exhaustive matching to catch real match bugs.
- **All error types must use `thiserror`.** No manual `Display` or `Error` impls on error enums/structs. `thiserror` derives are the single source of truth for error formatting and source chains.
- **Input validation errors are not corruption errors.** `EventTypeTooLong` (input error) mapped to `CorruptValue` (data error) causes wrong remediation. Use distinct variants.
- **Adapter error types must be distinct from facade error types.** `InMemoryStore::Error = StoreError` causes `StoreError::Adapter(Box::new(StoreError::...))` double-wrapping.
- **Provide `From` impls** for errors that cross crate boundaries at known mapping points. Don't manually map in 4+ places with identical code.
- **`ErrorId` truncation must be visually signaled** (e.g., append `...` if truncated).

### 4. API Design

- **No unused generic parameters or associated types.** If `M` is always `()` and `Aggregate::Error` is never used in any method, remove them. Add when the second concrete use case arrives. YAGNI.
- **Internal wire format helpers must be `pub(crate)`, not `pub`.** Encoding/decoding functions, size constants, and internal error types that don't appear in the public API must not be accessible to downstream crates.
- **`pub mod` leaks internals.** Use `mod` (private) with controlled `pub use` re-exports. `pub mod encoding` exposes every function in the module.
- **`#[doc(hidden)]` is not access control.** Test-only methods (`set_next_stream_id_for_testing`) must be `#[cfg(test)]` or `#[cfg(feature = "testing")]`.
- **Typestate builder intermediates** (`WithStreamId`, `WithVersion`, etc.) should not be independently constructable by users. Use `pub(crate)` on constructors or seal the module.
- **Redundant data must be eliminated or validated.** If `append(stream_id, envelopes)` takes `stream_id` AND each envelope contains `stream_id`, either remove it from the envelope or validate consistency. Silent mismatch = data corruption.
- **Type asymmetries across read/write paths must be intentional and documented.** `PendingEnvelope.stream_id() -> &StreamId` vs `PersistedEnvelope.stream_id() -> &str` is confusing.
- **Panics are for programmer bugs, not data or capacity limits.** `apply()` panicking on `MAX_UNCOMMITTED` is wrong — it's a capacity limit hit by normal usage. Return `Result`. Panics on corrupt persisted data crash the process instead of surfacing recoverable errors.
- **Rust naming conventions matter.** `new_unchecked` means no validation (caller guarantees preconditions). If it `assert!`s and panics, it's `new`, not `new_unchecked`.
- **`pub(crate)` fields → constructor.** Don't construct structs via struct literal syntax from other modules. Provide a `pub(crate) fn new(...)` so adding a field doesn't break every construction site.
- **Trait contracts must document semantics.** `read_stream(from)` — is `from` inclusive or exclusive? `Version::INITIAL` (0) — is it a valid event version or only a sentinel? Document it on the trait, not in one implementation.
- **Each crate validates at its own boundary.** The store crate must not trust kernel guarantees (version >= 1) without its own check. The fjall crate must not trust store guarantees. Every crate defends itself.

### 5. Concurrency Safety

- **`Relaxed` memory ordering requires structural proof**, not a comment about external library behavior. If correctness depends on fjall's `write_tx()` serializing writers, that's an undocumented coupling. Use `Acquire`/`Release` or a mutex to make the invariant self-contained.

### 6. Code Style — Functional-First, Allocate-Last

- **Prefer combinators over imperative control flow.** Use `.map()`, `.and_then()`, `.map_or_else()`, `.filter()`, `.fold()`, and other iterator/`Option`/`Result` combinators instead of `if`/`else`/`match` when the transformation is a simple data flow. Reserve imperative style for cases where it measurably improves performance or enables compile-time safety that combinators cannot express.
- **Lazy over eager.** Use iterators and lazy chains instead of collecting into intermediate `Vec`s. Only `.collect()` when you need the collection as a concrete value. `stream.filter().map().take()` is preferred over `let mut v = Vec::new(); for x in stream { if cond { v.push(f(x)); } }`.
- **Borrow before own.** Default to `&T` and lifetimes. Only clone/allocate when borrowing is impossible (crossing `async` boundaries, returning owned data to callers, storing in long-lived containers). `Cow<'a, T>` bridges the gap when ownership is conditionally needed.
- **No gratuitous allocations.** Every `Vec::new()`, `.to_owned()`, `.to_string()`, `Box::new()`, or `.clone()` in a hot path must justify itself. Prefer stack allocation (`ArrayVec`, `SmallVec`, `[T; N]`) for bounded collections. Use `&str` over `String`, `&[u8]` over `Vec<u8>`.
- **Extension traits for GAT streams.** Our `EventStream` uses GAT lending iterators where standard `Iterator` combinators aren't available. Add extension trait methods (e.g., `EventStreamExt`) to keep call sites clean and composable rather than forcing every consumer into imperative `while let` loops.
- **`let ... else` over `if let ... else { return }`.**  When the `else` branch is an early return/error, `let ... else` eliminates a nesting level and makes the happy path primary.
- **Collapse redundant branches.** When both arms of an `if/else` compute the same expression except at a single boundary value, and the expressions produce identical results at that boundary, collapse them into one expression.
- **All `use` imports at the top of the file.** Never scatter `use` statements mid-file. Never use deep inline path qualification like `crate::error::AppendError::Conflict { .. }` in match arms or expressions — import the type at the top and use the short name. The only exception is a genuinely one-off reference where adding an import would be more confusing than the inline path.

### 7. Testing — 4 Cross-Cutting Categories (highest priority)

Every new feature MUST include tests in these 4 categories BEFORE any other test methodology:

1. **Sequence/Protocol Tests** — Multi-step interactions on the same object. Test all valid operation sequences, not just individual operations in isolation.
2. **Lifecycle Tests** — Create, close, corrupt, reopen. If it persists state, test write-close-reopen-verify, write-corrupt-reopen-detect, and write-crash-reopen-recover.
3. **Defensive Boundary Tests** — Feed each crate inputs that violate its upstream crate's guarantees.
4. **Linearizability/Isolation Tests** — Concurrent readers and writers with snapshot consistency assertions.

After the 4 categories above, apply the 21 testing methodologies (see test strategy docs).

### 8. Test Quality Rules

Every test must satisfy ALL of these:

- **Calls the actual SUT.** Don't reimplement production logic in the test and prove properties of the reimplementation. Don't write a custom `ProbeStore` when `InMemoryStore` or `FjallStore` exist. Call the real function with the real dangerous input.
- **Can actually fail.** If both branches of a conditional pass (`if empty { println!(...) } else { assert!(...) }`), the test is worthless. Every test must have an assertion that would fail if the invariant breaks.
- **Asserts the specific correct result**, not "something happened." `assert!(msg.contains('3'))` is not an assertion — it matches any string containing `3`. Use `assert_eq!` with exact expected values.
- **`println!` is not an assertion.** Data loss, corruption, and safety violations must be asserted on, not logged.
- **"Concurrent" tests must have actual concurrency.** `tokio::spawn` + `Barrier` to ensure operations overlap. Sequential-then-check is not a concurrency test.
- **Property test ranges must include boundaries.** Every proptest strategy must include `0`, `1`, `MAX-1`, `MAX` via `prop_oneof!` alongside the interior range. For strings/IDs: empty, max-length, max-length+1.
- **State machine tests must include concurrent mode** if the SUT is concurrent.
- **Bug probe tests must FAIL when the bug exists**, not pass. If a known bug is accepted, use `#[ignore]`, not a green test that documents the issue in comments.
- **Each invariant tested once in a canonical location.** Don't duplicate the same property test across 3-5 files with different domain types.
- **Benchmarks measure production code**, not test fixtures. Use official testing adapters, not hand-rolled ones. Separate setup from measurement — don't benchmark `tempdir() + open()` when you want to measure `append()`.
- **No `Box::leak` in proptest** without documentation and bounded iteration counts.

## Key Conventions

- **Rust edition 2024** with `rustfmt` edition 2024
- **Strict clippy**: `all`, `pedantic`, `nursery` denied; `unwrap_used`, `expect_used`, `panic`, `todo`, `as_conversions`, `shadow_*`, `allow_attributes_without_reason` all denied
- **Workspace dependencies**: all dependency versions declared in root `Cargo.toml` `[workspace.dependencies]`; crate-level Cargo.toml files use `workspace = true`
- **workspace-hack crate**: managed by `cargo-hakari` for build optimization; run `cargo hakari generate` after dependency changes
- **Commit style**: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`) with optional scope (e.g. `feat(fjall):`, `fix(store):`)
- **Dual license**: MIT OR Apache-2.0
- **Property-based tests** via `proptest` in kernel, store, and fjall crates
- **CI checks** (via Nix flake): clippy (deny warnings), fmt, taplo fmt, cargo-audit, cargo-deny, nextest, tarpaulin coverage (Linux only), hakari verification
