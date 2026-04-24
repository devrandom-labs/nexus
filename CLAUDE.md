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
nexus-fjall   --> nexus-store --> nexus (kernel)
nexus-macros <-- nexus (kernel, optional via "derive" feature)
```

### Kernel Crate (`nexus`) — Flat Module Layout

- **`aggregate.rs`** — `Aggregate` trait (binds State + Error + Id), `AggregateRoot<A>` (read-only state container with version tracking + `replay` + `advance_version` + `apply_events`), `AggregateState` trait (`initial()`, `apply(self, &Event) -> Self`; requires `Clone` for panic safety), `AggregateEntity` trait (newtype delegation pattern with default methods: `id`, `state`, `version`, `replay`), `Handle<C, const N: usize = 0>` trait (per-command decide function returning `Events<E, N>`; N declares max additional events, default 0 = single event). Configurable limit: `MAX_REHYDRATION_EVENTS` (default 1M, `NonZeroUsize`).
- **`version.rs`** — `Version` newtype over `NonZeroU64` (event versions always >= 1). `Version::new(u64) -> Option<Self>` (mirrors `NonZeroU64::new`), `Version::INITIAL` = 1, `Version::next() -> Option<Self>`. `VersionedEvent<E>` pairs an event with its version.
- **`event.rs`** / **`events.rs`** — `DomainEvent` trait (extends `Message`, provides `name() -> &'static str`). `Events<E, const N: usize = 0>` is an `ArrayVec`-backed collection guaranteeing at least one event with compile-time capacity N+1; constructed via `events![e1, e2]` macro. N=0 (default) = single event, no heap allocation, `no_std`/`no_alloc` compatible.
- **`error.rs`** — `KernelError`: `VersionMismatch { expected, actual }`, `RehydrationLimitExceeded { max }`, `VersionOverflow`. All `#[non_exhaustive]`.
- **`id.rs`** — `Id` marker trait requiring `Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static`. `AsRef<[u8]>` provides stable byte representation for storage keys (not `Display`, which is for humans).
- **`message.rs`** — `Message` base marker trait (`Send + Sync + Debug + 'static`).

### Store Crate (`nexus-store`) — Persistence Edge Layer

Organized into 4 module directories + 3 cross-cutting files:

- **`codec/`** — Serialization traits.
  - `owning.rs` — `Codec<E>` for owning encode/decode (serde-based).
  - `borrowing.rs` — `BorrowingCodec<E>` for zero-copy decode (rkyv, flatbuffers).
- **`envelope/`** — Event containers for read & write paths.
  - `pending.rs` — `PendingEnvelope<M>` (owned, write-path) built via typestate builder: `pending_envelope(version).event_type().payload().build(metadata)`.
  - `persisted.rs` — `PersistedEnvelope<'a, M>` (borrowed, read-path) returned by cursors.
- **`upcasting/`** — Schema evolution.
  - `morsel.rs` — `EventMorsel<'a>`: zero-copy-when-possible data unit flowing through transforms.
  - `upcaster.rs` — `Upcaster` trait for raw-byte schema migrations. `()` is the no-op passthrough.
- **`store/`** — Storage infrastructure: traits adapters implement + facades users interact with.
  - `store.rs` — `Store<S>`: `Arc`-wrapped shared handle to any `RawEventStore`. Clone-cheap. Factory methods: `repository(codec, upcaster) -> EventStore`, `zero_copy_repository(codec, upcaster) -> ZeroCopyEventStore`.
  - `raw.rs` — `RawEventStore<M>` trait: byte-level `append` and `read_stream`.
  - `stream.rs` — `EventStream<M>` trait: GAT lending cursor.
  - `event_store.rs` — `EventStore<S, C, U>` facade (owning codec). Holds `Store<S>`. Implements `Repository<A>`.
  - `zero_copy_event_store.rs` — `ZeroCopyEventStore<S, C, U>` facade (borrowing codec). Holds `Store<S>`. Implements `Repository<A>`.
  - `repository.rs` — `Repository<A>` trait: high-level `load` and `save`.
- **`error.rs`** — `StoreError`, `AppendError<E>`, `UpcastError`, `InvalidSchemaVersion`.
- **`stream_label.rs`** — `StreamLabel` (diagnostic ID for error messages), `ToStreamLabel` (blanket impl for `Display`).
- **`testing.rs`** — `InMemoryStore` and `InMemoryStream` (feature-gated behind `testing`).

### Fjall Adapter Crate (`nexus-fjall`) — Embedded LSM-Tree Event Store

- **`store.rs`** — `FjallStore` implements `RawEventStore`. Two partitions: `streams` (point-read optimized metadata) and `events` (scan-optimized, LZ4 compressed). Maps string `StreamId` to numeric `u64` IDs for key-space efficiency. Atomic writes via fjall transactions. Overflow-safe numeric ID allocation.
- **`builder.rs`** — `FjallStoreBuilder`: `FjallStore::builder(path).streams_config(fn).events_config(fn).open()`.
- **`stream.rs`** — `FjallStream` implements `EventStream` as a lending cursor over eagerly-loaded rows.
- **`encoding.rs`** — Binary key/value encoding. Event keys: `[u64 BE stream_id][u64 BE version]` (16 bytes). Values: `[u32 LE schema_version][u16 LE event_type_len][event_type][payload]`.
- **`error.rs`** — `FjallError`: `Io`, `CorruptValue`, `CorruptMeta`, `StreamIdExhausted`.

### Aggregate Lifecycle (Key Flow)

1. Define aggregate via `#[nexus::aggregate(state = S, error = E, id = I)]` on a unit struct, or implement `Aggregate` + `AggregateEntity` + `Handle<C, N>` manually
2. `Aggregate::new(id)` creates a fresh aggregate (delegates to `AggregateRoot::new`; version = `None`)
3. **Load (rehydration):** `root.replay(version, &event)` replays persisted events with strict version validation (must start at 1, strictly sequential, enforces `MAX_REHYDRATION_EVENTS`)
4. **Decide:** `aggregate.handle(command) -> Result<Events<E, N>, Error>` — pure decision function, reads state via `&self`, returns decided events without mutating. N is the const generic capacity declared on `Handle<C, N>` (default 0 = single event).
5. **Persist + advance:** Repository writes events to store, then calls `root.advance_version(new_version)` + `root.apply_events(&events)` to sync in-memory state

### `nexus-macros` — Proc Macros

Two macros:
- **`#[nexus::aggregate(state = S, error = E, id = I)]`** — Attribute macro on a unit struct. Generates `Aggregate` impl, `AggregateEntity` impl, newtype wrapping `AggregateRoot`, `new(id)` constructor, and redacted `Debug` (shows only id + version).
- **`#[derive(DomainEvent)]`** — Derive macro for enums only. Generates `Message` + `DomainEvent` impls, with `name()` returning variant names as `&'static str`.

### Examples

- **`inmemory`** — Pure in-memory event sourcing, no persistence (bank account domain)
- **`store-inmemory`** — Demonstrates all `nexus-store` traits with `InMemoryStore`, including codec, upcasting, and schema evolution
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
- **All public enums must be `#[non_exhaustive]`** — adding a variant should never be a breaking change.
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
