# nexus

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

**The foundational Rust crate for building uncompromising, type-safe, high-performance systems using DDD, ES, and CQRS.**

---

## Overview

Nexus provides the core building blocks and architectural guidance for developing sophisticated applications in Rust, strictly adhering to the principles of:

* **Domain-Driven Design (DDD):** Encouraging rich domain models and clear boundaries.
* **Event Sourcing (ES):** Persisting state as a sequence of immutable domain events.
* **Command Query Responsibility Segregation (CQRS):** Separating write-side command processing from read-side queries.
* **Hexagonal Architecture (Ports and Adapters):** Decoupling the application core from infrastructure concerns.
* **Clean Code:** Promoting testable, maintainable, and understandable code.

Nexus aims to be the gold standard for implementing these patterns idiomatically in Rust, leveraging the language's strengths – particularly its powerful type system and concurrency primitives – to their absolute limit.

## Core Philosophy & Goals

Nexus is being built with an uncompromising focus on:

* 🚀 **Performance & Efficiency:** Aiming for near-zero overhead abstractions, efficient dispatch, and minimal allocations. Performance is paramount.
* 🔒 **Extreme Type Safety:** Leveraging Rust's type system (traits, generics, associated types, lifetimes) to maximize compile-time correctness and eliminate runtime errors. Invalid states should be unrepresentable where possible.
* 🏛️ **Architectural Purity & Power:** Strictly enforcing DDD, ES, CQRS, and Hexagonal principles. Utilizing advanced Rust features to achieve the objectively superior technical solution.
* ✨ **Ergonomic Public API (Future Goal):** While potentially complex internally, the crate's public API aims to be intuitive and require minimal boilerplate for users implementing the target architectures.
* 튼튼 **Production-Grade Robustness:** Designed for thread-safety, rigorous testability (especially isolating domain logic), and the demands of high-throughput systems.
* 🗼 **`tower` Integration (Planned):** Intends to leverage the `tower` ecosystem (`Service`, `Layer`) for composable, high-performance middleware and service abstraction around command/query handlers.

## ⚠️ Current Status (April 2025) ⚠️

**Nexus is currently in the early design and active development phase. It is NOT yet ready for production use.**

* **Focus:** Defining and solidifying the core abstractions for commands, events, aggregate state management (`AggregateState`, `AggregateType`, `AggregateRoot`), and command handling logic (`AggregateCommandHandler`). The design emphasizes type safety and testability, incorporating decisions like manual async handling (via `Pin<Box<dyn Future>>`) for explicit control.
* **Next Steps:** Finalizing core aggregate tests, defining the `EventSourcedRepository` port, designing the command/query dispatcher mechanism, implementing infrastructure adapters, and developing the query side.
* **Contributions:** Welcome! See the Contributing section.

## Core Concepts (Design In Progress)

* **Messages:** `Command`, `Query`, `DomainEvent` traits define the core message types, using associated types (`Result`, `Error`) for strong contracts.
* **Aggregate:** Decomposed into:
    * `AggregateState`: Holds state data, evolves via `apply(event)`.
    * `AggregateType`: Compile-time marker linking `Id`, `State`, `Event` types.
    * `AggregateRoot<AT>`: Manages state instance, version, uncommitted events; orchestrates command execution via `execute`.
* **Command Handling:**
    * `AggregateCommandHandler<C, Services> { type State; ... }`: Trait encapsulating pure domain logic for a specific command `C` and services `Services`, tied to a specific `State` type. Enables grouping related command logic while ensuring handlers are scoped to a single aggregate type. Uses manual async (`Pin<Box<dyn Future>>`) for explicit control.
* **Persistence (Planned):**
    * `EventSourcedRepository<AT>`: Trait (Port) defining `load` and `save` operations for `AggregateRoot<AT>` instances.
* **Dispatch & Middleware (Planned):**
    * A central dispatcher will route commands/queries.
    * Intends to use `tower::Service` for outer command/query handlers and `tower::Layer` for middleware (transactions, logging, metrics, validation, etc.).

## Usage (Placeholder)

```rust
// Usage examples will be added here once the core APIs stabilize.
// See the `examples/` directory in the repository (coming soon).

// Example structure (conceptual)
// 1. Define AggregateType, State, Event, Commands, Command Handlers
// 2. Implement AggregateState for State
// 3. Implement AggregateCommandHandler for logic handlers
// 4. Implement EventSourcedRepository for persistence
// 5. Configure Dispatcher and Middleware
// 6. Dispatch Commands / Queries
