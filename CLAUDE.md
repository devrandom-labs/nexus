# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nexus is an event-sourcing and DDD kernel for Rust. It provides composable traits, derive macros, and infrastructure adapters (starting with fjall). The project is **experimental** with an unstable API.

## Build & Development Commands

**Prerequisites:** Nix with flakes enabled. Use `nix develop` (or `direnv allow`) to enter the dev shell.

```bash
# Build everything
cargo build --all

# Run all tests
cargo test --all

# Run tests for a specific crate
cargo test -p nexus
cargo test -p nexus-store
cargo test -p nexus-fjall
cargo test -p nexus-macros

# Run a single test by name
cargo test -p nexus-store -- envelope_tests

# Check formatting
cargo fmt --all --check

# Apply formatting
cargo fmt --all

# Lint (CI runs with --deny warnings)
cargo clippy --all-targets -- --deny warnings

# Format TOML files (taplo)
taplo fmt

# Check workspace-hack crate is up-to-date
cargo hakari generate --diff
cargo hakari manage-deps --dry-run
cargo hakari verify
```

## Architecture

### Crate Dependency Graph

```
nexus-fjall   --> nexus-store --> nexus (kernel)
nexus-macros <-- nexus (kernel, optional via "derive" feature)
```

### Kernel Crate (`nexus`) — Flat Module Layout

- **`aggregate.rs`** — `Aggregate` trait (binds State + Error + Id), `AggregateRoot<A>` (event-sourced container with version tracking), `AggregateState` trait (`initial()`, `apply(self, &Event) -> Self`, `name()`), `AggregateEntity` trait (newtype delegation pattern). Configurable limits: `MAX_UNCOMMITTED` (default 1024), `MAX_REHYDRATION_EVENTS` (default 1M).
- **`stream_id.rs`** — `StreamId` opaque newtype with validated construction. `StreamId::new(name, id)` validates ASCII alphanumeric + underscore in name, formats as `"{name}-{id}"`. `StreamId::from_persisted(s)` for lenient backward-compatible loading. Max 512 bytes. Error type: `InvalidStreamId`.
- **`version.rs`** — `Version` newtype over `u64` (monotonic event sequence numbers). `VersionedEvent<E>` pairs an event with its version.
- **`event.rs`** / **`events.rs`** — `DomainEvent` trait (extends `Message`, provides `name() -> &'static str`). `Events<E>` is a `SmallVec`-backed collection guaranteeing at least one event; constructed via `events![e1, e2]` macro.
- **`error.rs`** — `KernelError` (version mismatch, rehydration limit). `ErrorId` is a stack-allocated 64-byte buffer for zero-alloc error paths.
- **`id.rs`** — `Id` marker trait requiring `Clone + Send + Sync + Debug + Hash + Eq + Display + 'static`.
- **`message.rs`** — `Message` base marker trait (`Send + Sync + Debug + 'static`).

### Store Crate (`nexus-store`) — Persistence Edge Layer

- **`raw.rs`** — `RawEventStore<M>` trait: byte-level `append` and `read_stream` that database adapters implement. Accepts `&StreamId`.
- **`stream.rs`** — `EventStream<M>` trait: GAT lending cursor (`next()` returns `PersistedEnvelope<'_, M>` borrowing from the cursor buffer).
- **`envelope.rs`** — `PendingEnvelope<M>` (owned, write-path) built via typestate builder: `pending_envelope(StreamId).version().event_type().payload().build(metadata)`. `PersistedEnvelope<'a, M>` (borrowed, read-path) returned by cursors.
- **`event_store.rs`** — `EventStore<S, C, U>` and `ZeroCopyEventStore<S, C, U>` facades composing a `RawEventStore` + `Codec`/`BorrowingCodec` + `UpcasterChain`. Both implement `Repository<A>`.
- **`repository.rs`** — `Repository<A>` trait: high-level `load(&self, stream_id, id)` and `save(&self, stream_id, aggregate)`.
- **`codec.rs`** / **`borrowing_codec.rs`** — `Codec<E>` for owning encode/decode (serde-based). `BorrowingCodec<E>` for zero-copy decode (rkyv, flatbuffers).
- **`upcaster.rs`** / **`upcaster_chain.rs`** — `EventUpcaster` trait for raw-byte schema migrations. `UpcasterChain` + `Chain<H, T>` for compile-time monomorphized chaining.
- **`error.rs`** — `StoreError`, `AppendError<E>` (optimistic concurrency conflicts), `UpcastError`, `InvalidSchemaVersion`.
- **`testing.rs`** — `InMemoryStore` and `InMemoryStream` (feature-gated behind `testing`).

### Fjall Adapter Crate (`nexus-fjall`) — Embedded LSM-Tree Event Store

- **`store.rs`** — `FjallStore` implements `RawEventStore`. Two partitions: `streams` (point-read optimized metadata) and `events` (scan-optimized, LZ4 compressed). Maps string `StreamId` to numeric `u64` IDs for key-space efficiency. Atomic writes via fjall transactions. Overflow-safe numeric ID allocation.
- **`builder.rs`** — `FjallStoreBuilder`: `FjallStore::builder(path).streams_config(fn).events_config(fn).open()`.
- **`stream.rs`** — `FjallStream` implements `EventStream` as a lending cursor over eagerly-loaded rows.
- **`encoding.rs`** — Binary key/value encoding. Event keys: `[u64 BE stream_id][u64 BE version]` (16 bytes). Values: `[u32 LE schema_version][u16 LE event_type_len][event_type][payload]`.
- **`error.rs`** — `FjallError`: `Io`, `CorruptValue`, `CorruptMeta`, `StreamIdExhausted`.

### Aggregate Lifecycle (Key Flow)

1. Define aggregate via `#[nexus::aggregate(state = S, error = E, id = I)]` on a unit struct, or implement `Aggregate` + `AggregateEntity` manually
2. `Aggregate::new(id)` creates a fresh aggregate (delegates to `AggregateRoot::new`)
3. `aggregate.apply(event)` records + applies events atomically (event stored before state mutation)
4. `aggregate.take_uncommitted_events()` drains events for persistence, advances persisted version
5. `AggregateRoot::replay(version, &event)` for rehydration from stored events with strict version validation

### `nexus-macros` — Proc Macros

Two macros:
- **`#[nexus::aggregate(state = S, error = E, id = I)]`** — Attribute macro on a unit struct. Generates `Aggregate` impl, `AggregateEntity` impl, newtype wrapping `AggregateRoot`, `new(id)` constructor, and redacted `Debug` (shows only id + version).
- **`#[derive(DomainEvent)]`** — Derive macro for enums only. Generates `Message` + `DomainEvent` impls, with `name()` returning variant names as `&'static str`.

### Examples

- **`inmemory`** — Pure in-memory event sourcing, no persistence (bank account domain)
- **`store-inmemory`** — Demonstrates all `nexus-store` traits with `InMemoryStore`, including codec, upcasting, and schema evolution
- **`store-and-kernel`** — Full lifecycle integrating kernel + store: create → apply → encode → persist → read → decode → rehydrate

## Key Conventions

- **Rust edition 2024** with `rustfmt` edition 2024
- **Strict clippy**: `all`, `pedantic`, `nursery` denied; `unwrap_used`, `expect_used`, `panic`, `todo`, `as_conversions`, `shadow_*`, `allow_attributes_without_reason` all denied
- **Workspace dependencies**: all dependency versions declared in root `Cargo.toml` `[workspace.dependencies]`; crate-level Cargo.toml files use `workspace = true`
- **workspace-hack crate**: managed by `cargo-hakari` for build optimization; run `cargo hakari generate` after dependency changes
- **Commit style**: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`) with optional scope (e.g. `feat(fjall):`, `fix(store):`)
- **Dual license**: MIT OR Apache-2.0
- **Property-based tests** via `proptest` in kernel, store, and fjall crates
- **CI checks** (via Nix flake): clippy (deny warnings), fmt, taplo fmt, cargo-audit, cargo-deny, nextest, tarpaulin coverage (Linux only), hakari verification
