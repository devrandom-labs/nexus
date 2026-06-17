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

**Zero-copy read path.** One `Decode<E>` trait with an `Output<'a>` GAT covers both owning codecs (serde JSON/bincode/postcard return `E`) and borrowing codecs (rkyv returns `&'a Archived<E>`, bytemuck returns `&'a E`). Event streams are `futures::Stream<Item = Result<PersistedEnvelope, _>>`, so the entire `futures::StreamExt` / `TryStreamExt` combinator surface is free. The on-disk row format aligns every event payload to a 16-byte boundary, so zero-copy decoders (rkyv, flatbuffers, `#[repr(C)]` POD) get sound `&T` references without a copy or a realignment step.

## Crates

| Crate | Description |
|-------|-------------|
| [`nexus`](crates/nexus) | Kernel — aggregates, events, versioning, command handling |
| [`nexus-macros`](crates/nexus-macros) | Derive macros — `DomainEvent`, `#[aggregate]`, `#[transforms]` |
| [`nexus-store`](crates/nexus-store) | Persistence edge — codecs, event streams, upcasters, repositories |
| [`nexus-fjall`](crates/nexus-fjall) | Embedded LSM-tree event store adapter (fjall) |

Projection is provided as primitives (`Projector`, `PersistTrigger`, `Subscription`, `SnapshotStore`); nexus ships no event-loop runner — the loop is the consumer's. See `examples/projection-tokio`.

## Features

- Schema evolution via `#[nexus::transforms]` upcasters
- Optimistic concurrency with version-checked appends
- Allocation-free errors (`ArrayString`-based, no heap on error paths)
- Aggregate snapshots with `AggregateRoot::restore`
- Pluggable codecs via cargo features on `nexus-store`: `serde` + `json`, `bytemuck` (`#[repr(C)]` POD), `rkyv` (archived zero-copy)
- 16-byte payload alignment as a wire-format invariant — sound zero-copy decode for rkyv, flatbuffers, and `#[repr(C)]` types out of the box
- Owned `bytes::Bytes` envelopes — event streams are plain `futures::Stream`, so every `futures::StreamExt` / `TryStreamExt` combinator works
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
