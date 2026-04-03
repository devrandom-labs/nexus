# Nexus

> A zero-compromise, **event-sourcing** & **CQRS** framework for Rust that puts type-safety and performance first.

[![crate](https://img.shields.io/crates/v/nexus.svg)](https://crates.io/crates/nexus)
[![docs](https://img.shields.io/docsrs/nexus/latest)](https://docs.rs/nexus)
[![license](https://img.shields.io/crates/l/nexus)](LICENSE)

Nexus helps you build robust, evolvable applications with first-class support for:

* **Domain-Driven Design (DDD)**
* **Event Sourcing (ES)**
* **Command Query Responsibility Segregation (CQRS)**
* **Hexagonal Architecture**

## The Kernel

The kernel is the innermost layer of Nexus — a pure, synchronous, zero-dependency core that provides maximum compile-time type safety.

### Design Principles

- **Concrete event enums** — no `Box<dyn>`, no runtime downcasting. The compiler enforces exhaustive event handling via `match`.
- **Marker-trait aggregates** — `Aggregate` binds `State`, `Error`, and `Id` at the type level. `AggregateRoot<A>` is parameterized by a single generic.
- **Encapsulated versioning** — `Version` and `VersionedEvent` cannot be forged. The kernel controls all version assignment.
- **Minimal error surface** — `KernelError` has a single variant (`VersionMismatch`). Infrastructure errors belong in outer layers.

### Quick Example

```rust
use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;

// 1. Define events
#[derive(Debug, Clone)]
struct UserCreated { name: String }

#[derive(Debug, Clone)]
struct UserActivated;

#[derive(Debug, Clone, nexus::DomainEvent)]
enum UserEvent {
    Created(UserCreated),
    Activated(UserActivated),
}

// 2. Define state
#[derive(Default, Debug)]
struct UserState { name: String, active: bool }

impl AggregateState for UserState {
    type Event = UserEvent;
    fn apply(&mut self, event: &UserEvent) {
        match event {
            UserEvent::Created(e) => self.name = e.name.clone(),
            UserEvent::Activated(_) => self.active = true,
        }
    }
    fn name(&self) -> &'static str { "User" }
}

// 3. Define aggregate
struct UserAggregate;

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

// 4. Use it
let mut user = AggregateRoot::<UserAggregate>::new(id);
user.apply(UserEvent::Created(UserCreated { name: "Alice".into() }));
user.apply(UserEvent::Activated(UserActivated));

let events = user.take_uncommitted_events();
assert_eq!(events.len(), 2);
```

### Verification

The kernel is tested with 10 different verification techniques:

| Technique | What it proves |
|-----------|---------------|
| Unit tests + edge cases | Correct behavior, caught a real version-tracking bug |
| Property-based testing (proptest) | 8 algebraic properties hold for all random inputs |
| Compile-failure tests (trybuild) | Invalid code fails to compile (type safety works) |
| Static assertions | Send, Sync, size, trait bounds enforced at compile time |
| Miri | Zero undefined behavior under strict provenance |
| Mutation testing (cargo-mutants) | 100% kill rate — every viable mutation caught |
| Contract invariants (debug_assert!) | Version arithmetic verified in debug builds |
| Benchmarks (criterion) | 15ns/event apply, 4.1us/10K event replay |
| Doc tests | All examples compile and run |
| Architecture tests | Kernel imports nothing from outer layers |

## Development

Prerequisites: [Nix](https://nixos.org/) with flakes enabled.

```bash
nix develop
cargo test -p nexus --test kernel
cargo bench --bench kernel_bench -p nexus
```

## License

Licensed under MIT OR Apache-2.0.
