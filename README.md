# Nexus

> A zero-compromise, **event-sourcing** & **CQRS** framework for Rust that puts type-safety and performance first.

[![crate](https://img.shields.io/crates/v/nexus.svg)](https://crates.io/crates/nexus)
[![docs](https://img.shields.io/docsrs/nexus/latest)](https://docs.rs/nexus)
[![license](https://img.shields.io/crates/l/nexus)](LICENSE)

Nexus helps you build robust, evolvable applications by giving you first-class support
for the following architectural patterns:

* **Domain-Driven Design (DDD)**
* **Event Sourcing (ES)**
* **Command Query Responsibility Segregation (CQRS)**
* **Hexagonal / Onion Architecture**

Instead of reinventing these ideas for every project, Nexus offers a set of composable
traits, derive-macros and infrastructure adapters that let you focus on *your* domain code.

## Workspace layout

This repository is a Cargo workspace that hosts several crates:

| Crate | Crates.io | Purpose |
|-------|-----------|---------|
| `nexus` | [link](https://crates.io/crates/nexus) | Core abstractions (aggregates, commands, queries, events, repositories) |
| `nexus-macros` | [link](https://crates.io/crates/nexus-macros) | Derive macros that remove boilerplate (`Command`, `Query`, `DomainEvent`) |
| `nexus-rusqlite` | [link](https://crates.io/crates/nexus-rusqlite) | A reference event-store implementation backed by SQLite |
| `nexus-test-helpers` | – | Utilities and generators for testing Nexus-powered code |
| `workspace-hack` | – | Internal crate used to satisfy Cargo resolver quirks |

## Getting started

Add Nexus to your project:

```toml
[dependencies]
nexus = "0.1"
nexus-macros = "0.1"          # optional, but highly recommended
```

Define an aggregate, some commands, and start dispatching:

```rust
use nexus::prelude::*;
use uuid::Uuid;
use nexus_macros::{Command, DomainEvent};

#[derive(Command)]
#[command(result = "()", error = "UserError")]
pub struct RegisterUser {
    pub id: Uuid,
    pub email: String,
}

#[derive(DomainEvent)]
#[domain_event(name = "UserRegistered")]
pub struct UserRegistered { /* fields omitted */ }

// ...
```

More complete examples will be available in the `examples/` directory once the API
stabilises.

## Status

Nexus is **experimental**. The public API is unstable and will change without notice
until we hit `v0.1.0`.

## Roadmap

- [ ] Stabilise aggregate and repository traits  
- [ ] Command / query dispatcher built on `tower`  
- [ ] Pluggable middleware (validation, tracing, transactions)  
- [ ] Additional persistence adapters (Postgres, DynamoDB, Kafka)  

## Contributing

Contributions, bug reports and suggestions are *very* welcome!  
Check the [contribution guidelines](CONTRIBUTING.md) (WIP) for details.

## License

Licensed under the Apache License, Version 2.0 (see `LICENSE`).
