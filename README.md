# Nexus

Event sourcing for Rust — no `Box<dyn>`, no runtime downcasting, no hidden allocations.

[![CI](https://github.com/devrandom-labs/nexus/actions/workflows/checks.yml/badge.svg)](https://github.com/devrandom-labs/nexus/actions/workflows/checks.yml)
[![license](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

```rust
use nexus::*;

#[derive(Debug, Clone, DomainEvent)]
enum Event {
    Deposited(Deposited),
    Withdrawn(Withdrawn),
}

#[derive(Debug, Clone)]
struct Deposited { amount: u64 }
#[derive(Debug, Clone)]
struct Withdrawn { amount: u64 }

#[derive(Default, Debug, Clone)]
struct Account { balance: u64 }

impl AggregateState for Account {
    type Event = Event;
    fn initial() -> Self { Self::default() }
    fn apply(mut self, event: &Event) -> Self {
        match event {
            Event::Deposited(e) => self.balance += e.amount,
            Event::Withdrawn(e) => self.balance -= e.amount,
        }
        self
    }
}

#[nexus::aggregate(state = Account, error = BankError, id = AccountId)]
struct BankAccount;

struct Deposit { amount: u64 }

impl Handle<Deposit> for BankAccount {
    fn handle(&self, cmd: Deposit) -> Result<Events<Event>, BankError> {
        Ok(events![Event::Deposited(Deposited { amount: cmd.amount })])
    }
}
```

## Why Nexus?

**Concrete event enums, not trait objects.** Events are plain Rust enums. `match` is exhaustive — the compiler catches every missing handler at build time. No `Box<dyn Any>`, no runtime downcasting, no message bus plumbing.

**`no_std` / `no_alloc` kernel.** The core crate has zero dependencies on `std` allocation. `Events<E, N>` uses `ArrayVec` with const generics — `N = 0` (the default) means a single event with no heap allocation at all. The kernel compiles for embedded and WASM targets.

**Zero-copy read path.** `BorrowingCodec` decodes events directly from database buffers via GAT lending iterators. No deserialization into owned structs on the read path — rkyv and flatbuffers plug in directly.

## Crates

| Crate | Description |
|-------|-------------|
| [`nexus`](crates/nexus) | Kernel — aggregates, events, versioning, command handling |
| [`nexus-macros`](crates/nexus-macros) | Derive macros — `DomainEvent`, `#[aggregate]`, `#[transforms]` |
| [`nexus-store`](crates/nexus-store) | Persistence edge — codecs, event streams, upcasters, repositories |
| [`nexus-fjall`](crates/nexus-fjall) | Embedded LSM-tree event store adapter (fjall) |

## Features

- Schema evolution via `#[nexus::transforms]` upcasters
- Optimistic concurrency with version-checked appends
- Allocation-free errors (`ArrayString`-based, no heap on error paths)
- Aggregate snapshots with `AggregateRoot::restore`
- Verified with proptest, miri, mutation testing, trybuild, and criterion

## Getting Started

Kernel only (pure domain logic, no persistence):

```toml
[dependencies]
nexus = { git = "https://github.com/devrandom-labs/nexus", features = ["derive"] }
```

With persistence (fjall embedded store):

```toml
[dependencies]
nexus = { git = "https://github.com/devrandom-labs/nexus", features = ["derive"] }
nexus-store = { git = "https://github.com/devrandom-labs/nexus" }
nexus-fjall = { git = "https://github.com/devrandom-labs/nexus" }
```

See the [examples](examples/) for complete working code:
- [`inmemory`](examples/inmemory) — pure in-memory event sourcing (bank account domain)
- [`store-inmemory`](examples/store-inmemory) — all store traits with `InMemoryStore`, including codec and upcasting
- [`store-and-kernel`](examples/store-and-kernel) — full lifecycle: create, decide, encode, persist, read, decode, rehydrate

## Status

Nexus is **experimental** with an unstable API. The kernel is well-tested (proptest, miri, mutation testing, trybuild). The store and fjall adapter are under active development.

## License

Licensed under your choice of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
