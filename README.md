# Nexus

> A zero-compromise **event-sourcing** & **CQRS** kernel for Rust.

[![crates.io](https://img.shields.io/crates/v/nexus.svg)](https://crates.io/crates/nexus)
[![docs.rs](https://img.shields.io/docsrs/nexus/latest)](https://docs.rs/nexus)
[![CI](https://github.com/devrandom-labs/nexus/actions/workflows/checks.yml/badge.svg)](https://github.com/devrandom-labs/nexus/actions/workflows/checks.yml)
[![license](https://img.shields.io/crates/l/nexus)](LICENSE-MIT)

Nexus is a pure, synchronous, zero-dependency kernel for building event-sourced applications in Rust. It provides maximum compile-time type safety with no `Box<dyn>`, no runtime downcasting, and no hidden allocations.

## Quick Start

```rust
use nexus::prelude::*;

// 1. Define your events
#[derive(Debug, Clone)]
enum UserEvent {
    Created { name: String },
    Activated,
}

impl DomainEvent for UserEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created { .. } => "UserCreated",
            Self::Activated => "UserActivated",
        }
    }
}

// 2. Define your state
#[derive(Default, Debug)]
struct UserState {
    name: String,
    active: bool,
}

impl AggregateState for UserState {
    type Event = UserEvent;

    fn initial() -> Self { Self::default() }

    fn apply(mut self, event: &UserEvent) -> Self {
        match event {
            UserEvent::Created { name } => self.name = name.clone(),
            UserEvent::Activated => self.active = true,
        }
        self
    }

    fn name(&self) -> &'static str { "User" }
}

// 3. Use it
let mut user = AggregateRoot::<UserAggregate>::new(id);
user.apply(UserEvent::Created { name: "Alice".into() });
user.apply(UserEvent::Activated);

let events = user.take_uncommitted_events();
assert_eq!(events.len(), 2);
```

## Design Principles

- **Concrete event enums** -- the compiler enforces exhaustive event handling via `match`. No `Box<dyn Any>`.
- **Marker-trait aggregates** -- `Aggregate` binds `State`, `Error`, and `Id` at the type level. One generic parameter.
- **Encapsulated versioning** -- `Version` and `VersionedEvent` cannot be forged. The kernel controls all version assignment.
- **Value-semantic state transitions** -- `apply(self, &Event) -> Self` consumes and returns state for atomic transitions.
- **Minimal error surface** -- `KernelError` has a single variant. Infrastructure errors belong in outer layers.

## Crates

| Crate | Description |
|-------|-------------|
| [`nexus`](https://crates.io/crates/nexus) | Core kernel -- aggregates, events, versioning |
| [`nexus-macros`](https://crates.io/crates/nexus-macros) | Derive macros (`DomainEvent`, `Command`, `Query`, `Aggregate`) |
| [`nexus-store`](https://crates.io/crates/nexus-store) | Event store edge layer -- codecs, streams, upcasters, repositories |

## Verification

The kernel is tested with 10 verification techniques:

| Technique | What it proves |
|-----------|---------------|
| Unit tests + edge cases | Correct behavior |
| Property-based testing (proptest) | Algebraic properties hold for all random inputs |
| Compile-failure tests (trybuild) | Invalid code fails to compile |
| Static assertions | Send, Sync, size, trait bounds enforced at compile time |
| Miri | Zero undefined behavior under strict provenance |
| Mutation testing (cargo-mutants) | Every viable mutation caught |
| Contract invariants (debug_assert!) | Version arithmetic verified in debug builds |
| Benchmarks (criterion) | Performance regression detection |
| Doc tests | All examples compile and run |
| Architecture tests | Kernel imports nothing from outer layers |

## Development

Prerequisites: [Nix](https://nixos.org/) with flakes enabled.

```bash
nix develop          # enter dev shell
cargo test --all     # run all tests
cargo clippy --all-targets -- --deny warnings
nix flake check      # full CI suite locally
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## Status

Nexus is **experimental** with an unstable API. The kernel layer is solid and heavily tested; the store layer is under active development.

## License

Licensed under your choice of:

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
