# Flatten Nexus Crate Design

**Date:** 2026-04-01
**Status:** Approved
**Scope:** Remove old modules, flatten kernel to top-level, add NexusId

## Goal

Make `nexus` = kernel + NexusId. Flat module structure, no nesting. Users write `use nexus::AggregateRoot`.

## Structure

```
crates/nexus/src/
├── lib.rs           re-exports everything
├── id.rs            Id trait
├── nexus_id.rs      NexusId (UUID v4)
├── message.rs       Message trait
├── event.rs         DomainEvent trait
├── version.rs       Version, VersionedEvent
├── aggregate.rs     AggregateState, Aggregate, AggregateRoot, EventOf
├── events.rs        Events collection, events! macro
└── error.rs         KernelError
```

## Changes

### Delete
- `src/domain/` — replaced by flat modules
- `src/command/` — moves to nexus-tower later
- `src/query/` — moves to nexus-tower later
- `src/event/` (old: PendingEvent, PersistedEvent, builder, metadata) — moves to nexus-store
- `src/store/` — moves to nexus-store
- `src/infra/` (EventId, CorrelationId move to nexus-store, NexusId stays)
- `src/serde/` — empty stubs, delete
- `src/kernel/` — contents promoted to top-level

### Workspace
- Exclude: `nexus-rusqlite`, `nexus-services`, `nexus-test-helpers`, `todo-rusqlite`
- Keep their directories — will migrate later

### nexus-macros
- Update paths: `::nexus::kernel::Message` → `::nexus::Message`
- Update paths: `::nexus::kernel::DomainEvent` → `::nexus::DomainEvent`
- Remove legacy struct DomainEvent derive (old modules gone)

### Dependencies (nexus Cargo.toml)
Keep: `smallvec`, `thiserror`, `uuid`
Remove: `async-trait`, `chrono`, `downcast-rs`, `serde`, `tokio-stream`, `tower`, `tracing`

### Tests
- Update all imports: `nexus::kernel::*` → `nexus::*`
- Update all imports: `nexus::kernel::aggregate::AggregateRoot` → `nexus::AggregateRoot`

### lib.rs
```rust
mod id;
mod nexus_id;
mod message;
mod event;
mod version;
mod aggregate;
mod events;
mod error;

pub use id::Id;
pub use nexus_id::NexusId;
pub use message::Message;
pub use event::DomainEvent;
pub use version::{Version, VersionedEvent};
pub use aggregate::{Aggregate, AggregateRoot, AggregateState, EventOf};
pub use events::Events;
pub use error::KernelError;

#[cfg(feature = "derive")]
pub use nexus_macros::DomainEvent;
```
