# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nexus is an event-sourcing, CQRS, and DDD framework for Rust. It provides composable traits, derive macros, and infrastructure adapters. The project is **experimental** with an unstable API.

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
nexus-store  --> nexus (core)
nexus-macros <-- nexus (core, optional via "derive" feature)
```

### Core Crate (`nexus`) - Layered by Module

- **`domain/`** - Core DDD primitives: `Aggregate`, `AggregateRoot<S, I>`, `AggregateState`, `Command`, `Query`, `DomainEvent`, `Id`, `Message`. The `AggregateState` trait requires `apply(&mut self, event)` for event-sourced state transitions and `name()` for stream naming.
- **`command/`** - Write-side CQRS: `AggregateCommandHandler` trait (async command handling against aggregate state + services), `EventSourceRepository` trait (load/save aggregates - the hexagonal "port"), `CommandHandlerResponse` (events + result).
- **`query/`** - Read-side CQRS: `ReadModel`, `ReadModelRepository` trait (port), `QueryHandlerFn` (adapts functions into `tower::Service`).
- **`event/`** - Event lifecycle types: `PendingEvent` (pre-persist, uses builder pattern), `PersistedEvent` (post-persist), `VersionedEvent` (version + boxed event), `EventMetadata`, `BoxedEvent` alias.
- **`store/`** - `EventStore` trait (append + read streams) and `EventStreamer` trait (read from checkpoint). These are the infrastructure ports.
- **`infra/`** - Infrastructure value types: `NexusId` (UUID v7-based), `EventId`, `CorrelationId`.
- **`serde/`** - Serialization support module.

### Aggregate Lifecycle (Key Flow)

1. `AggregateRoot::new(id)` or `AggregateRoot::load_from_history(id, events)` to create/rehydrate
2. `aggregate.execute(command, handler, services)` - handler produces events, state applies them, events become uncommitted
3. `aggregate.take_uncommitted_events()` - drain events for persistence
4. Version tracking enforces strict sequential ordering (sequence mismatch = error)

### `nexus-macros` - Derive Macros

Proc macros for `Command`, `Query`, and `DomainEvent`. Usage:
- `#[derive(Command)] #[command(result = "ResultType", error = "ErrorType")]`
- `#[derive(Query)] #[query(result = "ResultType", error = "ErrorType")]`
- `#[derive(DomainEvent)] #[domain_event(name = "EventName")]` (structs only, no enums/unions)

## Key Conventions

- **Rust edition 2024** with `rustfmt` edition 2024
- **Workspace dependencies**: all dependency versions declared in root `Cargo.toml` `[workspace.dependencies]`; crate-level Cargo.toml files use `workspace = true`
- **workspace-hack crate**: managed by `cargo-hakari` for build optimization; run `cargo hakari generate` after dependency changes
- **Commit style**: conventional commits (`feat:`, `fix:`, `docs:`)
- **Dual license**: MIT OR Apache-2.0
- **Property-based tests** via `proptest` in kernel and store tests
- **CI checks** (via Nix flake): clippy (deny warnings), fmt, taplo fmt, cargo-audit, cargo-deny, nextest, tarpaulin coverage, hakari verification
