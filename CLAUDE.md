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
nexus-store --> nexus (kernel)
nexus-fjall   --> nexus-store
nexus-macros <-- nexus (kernel, optional via "derive" feature)
```

### Kernel Crate (`nexus`) — Flat Module Layout

- **`aggregate.rs`** — `Aggregate` trait (binds State + Error + Id), implemented on a **bare marker** type (a unit struct, never instantiated — state lives in `AggregateRoot`). `AggregateRoot<A>` (read-only state container with version tracking + `replay` + `advance_version` + `apply_events`; the post-persist mutators are `#[doc(hidden)]` — driven by the repository, not user code) carries the inherent `handle<C, N>(&self, cmd)` **dispatch** that forwards to `A::handle(self.state(), cmd)`, so a loaded `AggregateRoot<A>` is directly decidable. `AggregateState` trait (`initial()`, `apply(self, &Event) -> Self`; requires `Clone` for panic safety). `Handle<C, const N: usize = 0>` trait (per-command **decide** function, `handle(state: &State, cmd) -> Events<E, N>`) — a pure function of `(state, command)`, implemented **on the marker**, with no access to version/identity (a decision never branches on persistence position); supertrait is `Aggregate`. There is **no entity newtype** and **no `AggregateEntity`/`from_root`** — `Handle` on the local marker coheres even for foreign command types, and the seam between "load returns `AggregateRoot`" and "decide" is dissolved by the dispatch method. Configurable limit: `MAX_REHYDRATION_EVENTS` (default 1M, `NonZeroUsize`). One `Handle<C, N>` per command type (two impls differing only in `N` for the same `C` are ambiguous at the dispatch call site).
- **`saga.rs`** — Saga / process-manager primitive, the **dual of an aggregate**. `Saga: Aggregate` supertrait adds `CorrelationKey` (instance routing key; bounds `Clone + Eq + Hash + Send + Sync + Debug + 'static`), `Command: Message` (thin outgoing intent vocabulary — enrichment to a target aggregate's fat command is the runtime's job, not nexus's), and `intent_for(&EventOf<Self>) -> Option<Command>` (Model A: commands are a 1:1 projection of the saga's own events, never a second output, so a dispatch can never drift from a recorded event). `React<E, N>: Saga` is the dual of `Handle<C, N>`: `react(state, &event) -> Result<Option<Events<EventOf, N>>, Error>` consumes an upstream event (the saga's "command" analogue — transient input, never folded) and produces the saga's own events (folded via the existing `AggregateState`). `Option` because a saga legitimately ignores events (`Ok(None)`); two ignore levels: `correlate -> None` (not routed) and `react -> Ok(None)` (routed, no-op). `correlate(&E) -> Option<CorrelationKey>` extracts the routing key (pure; instance *resolution* is the runtime's). One `React` impl per upstream event type. `AggregateRoot::react` dispatches like `AggregateRoot::handle` (a second inherent impl block). No new macro — `#[nexus::aggregate]` supplies the `Aggregate` half; `impl Saga` + `impl React` are hand-written. Store-side bounded saga repository (`load → react → save → return intents`) is a follow-up; the runtime loop/cursor/lifecycle/dispatch is Agency's. See `docs/plans/2026-06-18-saga-primitive-design.md`.
- **`version.rs`** — `Version` newtype over `NonZeroU64` (event versions always >= 1). `Version::new(u64) -> Option<Self>` (mirrors `NonZeroU64::new`), `Version::INITIAL` = 1, `Version::next() -> Option<Self>`. `VersionedEvent<E>` pairs an event with its version.
- **`event.rs`** / **`events.rs`** — `DomainEvent` trait (extends `Message`, provides `name() -> &'static str`). `Events<E, const N: usize = 0>` is an `ArrayVec`-backed collection guaranteeing at least one event with compile-time capacity N+1; constructed via `events![e1, e2]` macro. N=0 (default) = single event, no heap allocation, `no_std`/`no_alloc` compatible.
- **`error.rs`** — `KernelError`: `VersionMismatch { expected, actual }`, `RehydrationLimitExceeded { max }`, `VersionOverflow`.
- **`id.rs`** — `Id` marker trait requiring `Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static`. `AsRef<[u8]>` provides stable byte representation for storage keys (not `Display`, which is for humans). Default method `to_label() -> ArrayString<64>` provides allocation-free diagnostic labels for error messages.
- **`message.rs`** — `Message` base marker trait (`Send + Sync + Debug + 'static`).
- **`testing.rs`** (feature `testing`) — `AggregateFixture<A>` given/when/then test fixture. Drives the **real** replay path (`given` → `AggregateRoot::replay` with versions `1..=n`) then `AggregateRoot::handle` (`when`); typestate `AggregateFixture → Given → Acted<N>` makes illegal orderings non-compiling. `when` eagerly folds decided events into the root via the kernel's no-clone `apply_events`, so resulting-state assertions need no `Clone` bound (`AggregateState: Clone` was dropped). Assertions (all chainable, return `Self`): `then_expect_events` (`EventOf<A>: PartialEq + Debug`), `then_expect_error` (`Error: PartialEq`) / `then_expect_error_matching` (no bound), `then_expect_state` on both `Given` and `Acted` (no bound). No store/codec/serialization. Reached via `nexus::testing::AggregateFixture` (not re-exported at the crate root). Kernel feature flags: `default = []`, `derive`, `testing`. The fixture's own tests run under the default-feature `nix flake check` via a self dev-dependency (`nexus = { path = ".", features = ["testing", "derive"] }`) that unifies the features in — same pattern as nexus-store. See `docs/plans/2026-06-17-aggregate-test-fixture-design.md`. `SagaFixture<S>` (typestate `SagaFixture → SagaGiven → SagaReacted<N>`) is the saga dual: `given` (replay own history) → `when(&event)` (drives `AggregateRoot::react` + projects intents via `intent_for`) → `then_expect_events` / `then_expect_commands` / `then_expect_ignored` / `then_expect_error[_matching]` / `then_expect_state`. `correlate` is tested directly, not via the fixture. See `docs/plans/2026-06-18-saga-primitive-design.md`.

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
  - `Store<S>`: `Arc`-wrapped shared handle to any `RawEventStore`. Clone-cheap; carries no codec/upcaster/aggregate binding. Call `.repository()` to obtain a `RepositoryBuilder`. `.raw()` is the substrate escape hatch for users who need direct `RawEventStore` access; `#[cfg(feature = "subscription")] pub(crate) .arc()` exposes the inner `Arc<S>` to the subscription module (gated because nothing else needs it).
  - `RawEventStore` trait: byte-level `append`, `read_stream`, and `read_all`. `append` stamps each event with the next `GlobalSeq`. `read_stream` returns the adapter's concrete owned `Stream` associated type (a bounded per-stream scan from a `Version`); `read_all` returns the `AllStream` associated type (a bounded `$all` scan from a `GlobalSeq`). Both `from` bounds are **inclusive** — strict-after resume is the subscription loop's concern (`next_pos`), not the store's.
  - `GlobalSeq`: store-local global sequence number stamped on every event at append time — the `Version` analogue across *all* of one producer's streams, and the position an all-streams subscription resumes from. **Monotonic but not gapless** — an aborted append may burn values, so consumers must tolerate gaps.
- **Subscription machinery (`subscription.rs` / `wake.rs` / `catchup.rs` / `subscription_cursor.rs` / `notify.rs`)** — feature-gated under `subscription`. The catch-up-then-live-tail loop lives **here in nexus-store as ONE generic, no-`Box` machine**, reusable by *any* adapter that implements `RawEventStore + WakeSource` — the postgres-readiness win (the loop was extracted from `nexus-fjall`, which had grown a bespoke per-adapter copy; see `docs/plans/2026-06-22-nexus-fjall-cursor-unification-design.md` + `docs/plans/2026-06-23-nexus-fjall-cursor-unification-plan.md`).
  - `subscription.rs` — `Subscription<S: RawEventStore + WakeSource>`, the user-facing **generic builder**. Built via `Subscription::new(&store)` from a `Store<S>` handle (one `Arc` clone). `subscribe(&id, from)` / `subscribe_all(from)` are **synchronous and fallible**: they return `Result<impl Stream<Item = Result<PersistedEnvelope, <S as RawEventStore>::Error>> + Send, <S as WakeSource>::Error>` — register failure surfaces eagerly as the `Err`, read failures stream in-band as `Err` items (two distinct error domains, rule 3). The cursor **never returns `None`** (parks when caught up). `from: None` = beginning; `from: Some(v)` = events *strictly after* `v` (strict-after resume; no duplicate on reopen). The stream is `!Unpin` (it is the `unfold` of the live loop), so consumers `pin!` it — the zero-cost (no-`Box`) tradeoff. There is **no adapter-facing subscription trait** — the old `RawSubscription` / `RawAllSubscription` + `sealed` module were deleted; an adapter need only implement `RawEventStore` + `WakeSource`.
  - `wake.rs` — adapter-pluggable wake, used only as a generic bound (never `dyn`). `WakeSource: Send + Sync + 'static` with `register(&self, stream: Option<&[u8]>) -> Result<Self::Registration, Self::Error>` (`None` = `$all`) and `wake(&self, stream)` (called *after* a durable commit). `WakeRegistration` with `arm(&self) -> impl Future<Output = ()> + Send + 'static` — an RPITIT future (no associated `Wait` type, no boxing); the returned future is `'static` (carries no borrow of `self`) and lost-wakeup-safe (captures a "seen version" at `arm` time). In-process adapters use `StreamNotifiers`; distributed adapters (postgres) implement these over `LISTEN`/`NOTIFY`.
  - `catchup.rs` — `pub(crate)` `Catchup` seam fusing one subscription target's bounded position-keyed scan with a wait. Methods: `read_from(from)` (open a bounded scan from `from` INCLUSIVE), `arm()`, `position_of(env)`, and `next_pos(Option<Position>) -> Option<Position>` (the **single** resume transition: `None` → first position, `Some(v)` → strictly after `v` via `v.next()`, overflow → `None`). Two compile-time impls — `StreamCatchup` (per-stream, `Position = Version`) and `AllCatchup` (`$all`, `Position = GlobalSeq`) — let the one live loop monomorphize into two branch-free machines. `OwnedSubId` satisfies `Id`'s `'static` bound across reopened scans.
  - `subscription_cursor.rs` — the **single copy** of the arm-before-confirm-rescan lost-wakeup discipline: one generic `live<C: Catchup>(c, from) -> impl Stream` loop (RPIT, no `Box<dyn>`). Reopens its bounded scan every `CATCHUP_CHUNK` (1024) delivered rows so one adapter scan (and any GC watermark it pins) is never held across an unbounded backlog. On error it surfaces `Err` in-band and reopens from the last *successfully delivered* position on the next poll — no internal retry/dead-letter; recovery is the consumer's. A `C::Scan: Unpin` bound is kept method-local (not on the trait) because `StreamExt::next` needs it.
  - `notify.rs` — `StreamNotifiers`, the *in-process* per-stream wake registry, now impls `WakeSource` (`Registration = WakeReg`). Carries a `tokio::sync::watch` **generation** counter per stream (and one for `$all`) alongside its legacy `Notify` — `arm` clones a `watch::Receiver`, calls `mark_unchanged()` to pin the seen-version to the exact `arm` instant, then awaits `changed()` (only a *future* bump resolves it, closing the lost-wakeup window). Built via `Arc::new_cyclic` so it holds a `Weak<Self>` self-ref: the `register(&self)` trait method must build a drop-guard `SubscriptionGuard` that owns an `Arc<StreamNotifiers>` without an `&Arc<Self>` receiver. The inherent `wake` bumps the per-stream generation+`Notify` AND the `$all` watch generation (so a `WakeReg` armed on `$all` wakes), but *not* the legacy `$all` `Notify` (that path stays driven by `wake_all`, preserving the old test). Both the `watch`-generation and legacy `Notify`/`wake_all` paths live during the transition. Drop-guard lifecycle (entry exists iff ≥1 live subscriber, reaped synchronously at zero) is unchanged.
- **`stream.rs`** — `EventStream` marker trait over `futures::Stream<Item = Result<PersistedEnvelope, Self::Error>> + Send`. No methods of its own. The associated `Error` type is recovered from the stream's `Item` so call sites can bound on `S: EventStream<Error = MyErr>` instead of carrying an extra generic. All combinators come from `futures::StreamExt` / `TryStreamExt`. Replaced the GAT-lending `EventStream` + `BaseEventStream` + `EventStreamExt` + `OwnedEventStream` + `IntoStream` + `Map` + `TryMap` + `MapErr` + `TryScan` (~1370 lines deleted) — the owned `Bytes` envelope removed the lifetime cliff that motivated the GAT.
- **`wire.rs`** — One canonical frame builder used by every adapter: `wire::encode_frame(global_seq, schema_version, &event_type, &payload, metadata) -> Result<EncodedFrame, WireError>` and `wire::decode_frame(&[u8]) -> Result<DecodedFrame, DecodeError>`. Layout: `[u8 frame_format_version][u64 LE global_seq][u32 LE schema_version][u16 LE event_type_len][u32 LE meta_len][event_type][metadata?][padding][payload]` where `meta_len == u32::MAX` is the absent-metadata sentinel. The **leading `frame_format_version` byte (offset 0) is read first**: `decode_frame` decodes it via the typed `FrameFormatVersion` enum and branches on layout (`match header.format_version { V1 => decode_frame_v1(..) }`), rejecting an unknown byte as a typed `DecodeError::UnsupportedFrameVersion` rather than misparsing. It is the on-disk escape hatch that makes the frame **evolvable** (a future v2 can change alignment / add a CRC / store the payload length without a full data migration) — added in the pre-1.0-freeze window because the frame is irreversible once real data exists (#205); `HEADER_FIXED_SIZE` is `19` (version byte + the four fixed fields). The padding makes the payload pointer land on a **16-byte boundary** inside the returned `Bytes` — a wire-format invariant zero-copy decoders (rkyv default alignment, flatbuffers, `#[repr(C)]` POD up to 16) rely on; the version byte shifts the fixed fields one byte later but `align_padding` recomputes the payload position so the 16-byte boundary holds. Backing buffer built via `aligned_vec::AVec<u8, ConstAlign<16>>` → `bytes::Bytes::from_owner(avec)` for zero-copy ownership transfer that preserves alignment.
- **`state.rs`** — Unified snapshot persistence. One trait powers both projection state and aggregate snapshots.
  - `SnapshotStore<S, P>` trait: atomic persistence of derived state plus the position it was folded to. `hydrate(id, schema_version) -> Option<(P, S)>` and `commit(id, schema_version, position, &state)` — state and position are saved and loaded *together*, never separately, so a half-write is unrepresentable. Generic over the position type `P` (`Version` for a single stream, `GlobalSeq` for a multi-stream projection).
  - `CodecSnapshotStore<SS, C>`: bridges a byte-level `SnapshotStore<Vec<u8>, P>` (what fjall implements) to a typed `SnapshotStore<S, P>` via `Decode<S>` / `Encode<S>`. The position `P` passes through untouched. `CodecSnapshotStoreError::Wire` covers the rare wire-build failure when synthesizing an envelope from snapshot bytes.
  - `PersistTrigger` trait: `EveryNEvents(N)` (bucket-crossing), `AfterEventTypes(&[&str])` (semantic). Used by both projection runners and the snapshot decorator.
  - Inline `#[cfg(feature = "testing")] mod testing` block: `InMemorySnapshotStore`.
- **`projection.rs`** — Feature-gated under `projection`. Slim by design.
  - `Projector` trait: pure fallible fold function (`initial()` + `apply(state, &event) -> Result`). Fallibility is intentional: projections may do checked arithmetic where aggregates do not. Recovery policy (skip/fail/dead-letter) is the framework's concern, not the projector's. nexus ships no runner; the IO-driven loop is the consumer's. See `examples/projection-tokio`.
- **`repository.rs`** — Aggregate-facing trait + its two facade impls.
  - `Repository<A>` trait: high-level `load` and `save` for aggregates.
  - `EventStore<S, C, U>` facade — owning codec; HRTB bound `for<'a> C: Encode<E> + Decode<E, Output<'a> = E> + 'static`.
  - `ZeroCopyEventStore<S, C, U>` facade — borrowing codec; HRTB bound `for<'a> C: Encode<E> + Decode<E, Output<'a> = &'a E> + 'static`. Combined under one HRTB to satisfy `clippy::type_repetition_in_bounds` without weakening the constraint.
  - Both facades drive event-stream consumption through `futures::TryStreamExt::try_fold`. Closures use `Arc`-wrapped codec/upcaster captures so `FnMut` can move them.
  - `pub(crate) ReplayFrom<A>` trait shared with the `Snapshotting` decorator.
- **`saga.rs`** — Store-side bounded saga repository. `SagaRepository<S>: Repository<S>` extension trait, blanket-impl'd onto every repository (bare facade + `Snapshotting` decorator), with two provided methods: `react_and_save(&mut root, event)` (core: react → atomic save → project intents pinned to assigned versions; no load) and `dispatch(id, event)` (load + `react_and_save`, for stateless reactors). Returns `Reaction<S, N>` (`Ignored` | `Reacted { version, intents }`), where `intents` is a no-alloc `ProjectedIntents<S, N>` (ArrayVec, capacity `N+1`, `Events`-style first+rest) of `ProjectedIntent<S>` **capability tokens** (sealed `pub(crate)` constructor — possessing one proves the event is durable; carries `(saga_id, source_version)` dedup key, Model A). `SagaError<SagaErr, StoreErr>` (`React` | `Store` | `VersionOverflow`) with `is_conflict()` via the sealed `ConflictPredicate`. Conflict is **surfaced, never retried internally** (CLAUDE.md rule 5 — no dependency on Agency single-writer). Zero new persistence machinery — reuses `Repository::load`/`save`. See `docs/plans/2026-06-18-saga-repository-design.md`.
- **`builder.rs`** — `RepositoryBuilder` typestate builder with `!Send` marker `NeedsCodec`; produces `EventStore` or `ZeroCopyEventStore`. `NoSnapshot` is the default snapshot slot; `WithSnapshot<SS, T>` is the active slot (feature-gated: `snapshot`). Kept separate from `repository.rs` because it's pure typestate construction scaffolding (~390 lines).
- **`snapshot.rs`** — `Snapshotting<R, SS, T>` decorator (feature-gated: `snapshot`). Wraps an inner repository and transparently hydrates from a `SnapshotStore<A::State, Version>` on read, commits on write per a `PersistTrigger`. Snapshot save is best-effort (never blocks event persistence). Kept separate from `repository.rs` because it's the only feature-gated piece of the facade.
- **`error.rs`** — `StoreError<A, C, U>` (generic over adapter/codec/upcaster errors, allocation-free), `AppendError<E>`, `UpcastError<U>` (generic over transform error), `InvalidSchemaVersion`. All use `ArrayString<64>` for stream IDs instead of heap-allocated strings.
- **`testing.rs`** — `InMemoryStore`, `InMemoryStream`, `InMemoryStoreError` (feature-gated behind `testing`).

Feature flags: `serde`, `json` (implies `serde`), `snapshot`, `snapshot-json`, `projection`, `projection-json`, `subscription` (the generic catch-up + live-tail loop; pulls `tokio` + `foldhash` + `parking_lot`), `testing`.

Public sub-paths after the flatten: `nexus_store::codec::*`, `::envelope::*`, `::upcasting::*`, `::store::*`, `::state::*`, `::projection::*`, `::repository::*` (Repository + facades), `::builder::*` (builder typestate), `::snapshot::*` (decorator). All top-level types remain re-exported at `nexus_store::*` for convenience.

### Projection — primitives only, no runner

nexus deliberately ships **no** projection runner and **no** event loop. A read-model projection is built from four pure primitives that already live in `nexus-store`: `Projector` (the fallible fold), `PersistTrigger` (when to persist), `Subscription` (the cursor), and `SnapshotStore` (atomic `(state, position)` commit). The loop that wires them — and all runtime concerns (lifecycle, supervision, passivation, cursor management) — is owned by the consumer, never by nexus; shipping a loop here would make nexus a runtime and would duplicate logic that belongs to the runtime layer. In the Nexus + Agency product, that runtime is Agency's Zenoh-native actor framework (a kameo fork); a tokio reference loop lives in `examples/projection-tokio`. The earlier `nexus-framework` crate (a `tokio::select!` runner plus an Idle/Pending/Committed FSM) was retired on 2026-06-17 for exactly this reason — the FSM was loop bookkeeping, not an event-sourcing primitive.

### Fjall Adapter Crate (`nexus-fjall`) — Embedded LSM-Tree Event Store

The workspace pins `fjall = { version = "3", features = ["bytes_1"] }`. That feature is **load-bearing**: it flips fjall's `Slice` value to be a newtype over `bytes::Bytes` with zero-copy `From<Bytes>` / `From<Slice> for Bytes`. Without it every fjall read on the hot path would pay one alloc + memcpy to convert. With it, the `bytes::Bytes` inside `PersistedEnvelope` *is* the same Arc-counted buffer fjall handed us.

After the cursor unification (`docs/plans/2026-06-22-nexus-fjall-cursor-unification-design.md` + `docs/plans/2026-06-23-nexus-fjall-cursor-unification-plan.md`), `nexus-fjall` holds **only fjall-specific code**: the catch-up-then-live-tail loop now lives generically in `nexus-store` (see the subscription machinery above), so the four bespoke cursor files (`stream.rs`, `all_stream.rs`, `subscription_stream.rs`, `all_subscription_stream.rs` — `FjallStream` / `FjallAllStream` / `FjallSubscriptionStream` / `FjallAllSubscriptionStream`) and the old `VecDeque`/batch refill machinery were deleted.

- **`store.rs`** — `FjallStore` implements `RawEventStore` (`append` + `read_stream` + `read_all`, the latter two returning a `ScanCursor`), `WakeSource` (delegating `register`/`wake` to its `StreamNotifiers`; `wake` bumps both the per-stream and `$all` paths), and — under the `snapshot` feature — `SnapshotStore<Vec<u8>, Version>`. Partitions: `streams` (id bytes → version counter, point-read optimized), `events` (event rows, scan-optimized, LZ4 compressed), `global` (one key holding the store-wide `GlobalSeq` counter), and `snapshots` (under the `snapshot` feature). All writes go through one atomic fjall `write_tx`; `append` claims its `GlobalSeq` range inside the same transaction as the event writes. `append` is decomposed into helpers (`read_current_version`, `check_optimistic`, `read_current_global`) and uses running `checked_add` counters for the version sequence and `GlobalSeq` — no `unwrap_or(u64::MAX)` sentinels (rule 2). Each row is built via `nexus_store::wire::encode_frame` so the on-disk payload bytes land on a 16-byte boundary.
- **`scan.rs`** — fjall-private bounded read cursor (NOT exported; no other adapter shares fjall's on-disk key layout). `ScanStrategy` trait factors the only parts that differ between the per-stream (`Version`-keyed) and `$all` (`GlobalSeq`-keyed) reads — the keyset bound bytes (`lower_key`/`upper_key`) and how a stored row decodes into a `PersistedEnvelope`; `StreamScan` / `GlobalScan` are the two impls. `ScanCursor<S: ScanStrategy>` is ONE bounded cursor over a SINGLE lazy `fjall::Iter` (`keyspace.range(lower..=upper)`): fjall's range iterator is already a lazy k-way merge that pulls the next LSM block only when the current drains, so a single `Iter` bounds memory with no batching/refill needed. **Snapshot-consistent at `open`** — it pins one repeatable-read view as of open, so events appended after `open()` are not observed mid-scan (which is why a never-ending subscription opens a fresh `ScanCursor` per refill rather than holding one). Poisons (yields `None`) after the first error rather than silently skipping corrupt rows. `S: Unpin` bound (relied on by the generic live loop).
- **`builder.rs`** — `FjallStoreBuilder`: `FjallStore::builder(path).streams_config(fn).events_config(fn).open()`. The `batch_size` knob was removed — the single lazy `ScanCursor` makes a resident-row cap meaningless.
- **`partition.rs`** — Sealed `KeyspaceConfig` trait. Only `()` (defaults) and `FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions` closures implement it. Eliminates dynamic dispatch from the builder by monomorphising keyspace configuration at compile time.
- **`subscription_id.rs`** — private module holding `OwnedStreamId`, an owned byte-key wrapper that satisfies the `Id` `'static` bound when `read_stream` re-reads across the subscription loop's refills.
- **`wire_key.rs`** (renamed from `encoding.rs`, now a **private** `mod` — the old `pub mod encoding` rule-4 leak is fixed) — fjall key codecs only. Event keys: `[u16 BE id_len][id_bytes][u64 BE version]` (length-prefixed to prevent prefix collisions in range scans), plus the `$all` global key codec. Also holds `decode_event_value` (a thin wrapper over `nexus_store::wire::decode_frame`; the value layout is owned entirely by `nexus_store::wire`) and the `DecodeError` / `EncodeError` types. The test-only `encode_event_value` was deleted; white-box codec tests were relocated in-crate, and benches encode via `nexus_store::wire::encode_frame`.
- **`snapshot.rs`** — private module (feature `snapshot`) holding the fjall-private snapshot blob codec `[u32 LE schema_version][u64 BE version][payload]` — distinct from the event wire format, used only by the `SnapshotStore<Vec<u8>, Version>` impl in `store.rs`.
- **`error.rs`** — `FjallError`: `Io`, `CorruptValue`, `EnvelopeCorrupt` (envelope integrity, distinct from wire-format `CorruptValue`), `CorruptMeta`, `InvalidInput`, `VersionOverflow`, `GlobalSeqOverflow`, `Subscription` (`#[from] nexus_store::notify::NotifyError` — the `WakeSource::Error` surface, in practice only the unreachable subscriber-count overflow). The fake `InvalidSize` sentinel and the `MetadataOutOfBounds` variant were deleted. All diagnostic fields use `ArrayString<64>` / `ArrayString<128>` — no heap allocation on error paths.

### Aggregate Lifecycle (Key Flow)

1. Define the aggregate marker via `#[nexus::aggregate(state = S, error = E, id = I)]` on a unit struct (emits `impl Aggregate`), or implement `Aggregate` manually; implement `Handle<C, N>` on the marker as `handle(state, cmd)`
2. `AggregateRoot::<A>::new(id)` creates a fresh aggregate (version = `None`)
3. **Load (rehydration):** `root.replay(version, &event)` replays persisted events with strict version validation (must start at 1, strictly sequential, enforces `MAX_REHYDRATION_EVENTS`) — driven by the repository
4. **Decide:** `root.handle(command) -> Result<Events<E, N>, Error>` dispatches to `Aggregate::handle(root.state(), command)` — a pure decision function of `(state, command)`, returns decided events without mutating. N is the const generic capacity declared on `Handle<C, N>` (default 0 = single event).
5. **Persist + advance:** Repository writes events to store, then calls `root.advance_version(new_version)` + `root.apply_events(&events)` to sync in-memory state

### `nexus-macros` — Proc Macros

Three macros:
- **`#[nexus::aggregate(state = S, error = E, id = I)]`** — Attribute macro on a unit struct. Generates `impl Aggregate` plus a convenience `Name::new(id) -> AggregateRoot<Self>` constructor; the struct stays a bare marker. Implement `Handle<C>` on the marker. (No newtype, no `AggregateEntity`, no generated `Debug`.)
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
- **Toolchain & MSRV**: pinned to an exact **stable** release in `rust-toolchain.toml` (`channel`), which is the single source of truth for both `rustup` users and the Nix flake (consumed via fenix `fromToolchainFile`). **No nightly** — the crates carry no `#![feature(...)]` gates and the whole workspace (build, test, clippy, fmt, coverage, examples) runs on the pinned stable. MSRV policy: `rust-version` in `[workspace.package]` **equals** the pinned toolchain; when bumping Rust, bump `rust-toolchain.toml` *and* `rust-version` together (the pin tracks the latest stable we actually build/test against — no separate lower floor is claimed unless a CI job verifies it). All publishable crates opt in via `rust-version.workspace = true`.
- **Strict clippy**: `all`, `pedantic`, `nursery` denied; `unwrap_used`, `expect_used`, `panic`, `todo`, `as_conversions`, `shadow_*`, `allow_attributes_without_reason` all denied
- **Workspace dependencies**: all dependency versions declared in root `Cargo.toml` `[workspace.dependencies]`; crate-level Cargo.toml files use `workspace = true`
- **workspace-hack crate**: managed by `cargo-hakari` for build optimization; run `cargo hakari generate` after dependency changes
- **Commit style**: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`) with optional scope (e.g. `feat(fjall):`, `fix(store):`)
- **Dual license**: MIT OR Apache-2.0
- **Property-based tests** via `proptest` in kernel, store, and fjall crates
- **CI checks** (via Nix flake): clippy (deny warnings), fmt, taplo fmt, cargo-audit, cargo-deny, nextest, tarpaulin coverage (Linux only), hakari verification
