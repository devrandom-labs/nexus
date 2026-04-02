# Nexus Kernel Design

**Date:** 2026-03-30
**Status:** Approved
**Scope:** Core kernel layer — the innermost, dependency-free layer of the Nexus framework

## Goals

1. **Maximum compile-time type safety** — invalid states and operations are unrepresentable
2. **Great developer experience** — complexity hidden in derive macros, users write plain Rust
3. **Zero-cost abstractions** — no heap allocation for events, no vtable dispatch, no runtime downcasting

## Design Decisions

### D1: Events are concrete enums, not trait objects

**Decision:** `AggregateState::Event` is a concrete `Sized` enum type, not `Box<dyn DomainEvent>`.

**Rationale:** Exhaustive `match` in `apply()` enforced by compiler. No `Box` heap allocation. No `downcast-rs` runtime dispatch. Adding a new event variant forces handling everywhere at compile time.

**Removes:** `downcast-rs` dependency, `DowncastSync` bound on `Message`, `BoxedEvent` type alias, `impl_downcast!` macro usage.

### D2: `DomainEvent` trait exists, derive macro implements it on enums

**Decision:** `DomainEvent` trait provides `fn name(&self) -> &'static str`. `#[derive(DomainEvent)]` on an enum generates the impl with per-variant `match`.

**Rationale:** Trait gives an extension point for future methods. Derive macro eliminates boilerplate. Users never hand-write `impl DomainEvent`.

**Change from current:** `#[derive(DomainEvent)]` moves from individual structs to the wrapping enum.

### D3: `Aggregate` trait is a type-level specification (marker pattern)

**Decision:** `Aggregate` is implemented on a user-defined marker type (unit struct) that binds `State`, `Error`, and `Id` together. `AggregateRoot<A: Aggregate>` is parameterized by a single generic.

```rust
struct UserAggregate;

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = NexusId;
}
```

**Rationale:** Solves the orphan rule — users own `UserAggregate`, so they can `impl AggregateRoot<UserAggregate>` to add business logic methods directly. Single generic `A` is cleaner than `<S, I>` everywhere.

### D4: Event type derived from `AggregateState`, not declared on `Aggregate`

**Decision:** `Aggregate` trait has no `type Event`. The event type is always `<A::State as AggregateState>::Event`, accessed via `EventOf<A>` type alias.

**Rationale:** Single source of truth. Impossible to mismatch. Users declare the event type once on `AggregateState::Event`.

### D5: Business logic lives on `impl AggregateRoot<UserAggregate>`

**Decision:** Users write domain methods directly on `AggregateRoot<ConcreteAggregate>`. No `Command` trait, `Handler` trait, or `Services` in the kernel.

```rust
impl AggregateRoot<UserAggregate> {
    pub fn create(&mut self, name: String) -> Result<(), UserError> {
        // check invariants via self.state()
        // produce events via self.apply_event()
    }
}
```

**Rationale:** Business logic is tightly coupled to the aggregate — it should live there. No indirection through handler traits. `Command`, `Handler`, `Services` become optional application-layer patterns. The kernel stays pure.

### D6: `Id` trait has 6 pure-domain bounds

**Decision:**
```rust
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + 'static {}
```

**Removed:** `FromStr` (parsing is infrastructure), `AsRef<[u8]>` (serialization is infrastructure). Store adapters add these bounds where needed.

### D7: `Version` newtype replaces raw `u64`

**Decision:** Single `Version(u64)` newtype for all version/sequence concepts.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);
```

**Rationale:** Prevents mixing versions with arbitrary integers. Provides a place for `INITIAL`, `next()`, `Display`. Single type (not multiple newtypes) because event sequence and aggregate version are the same concept.

### D8: Kernel error is minimal — single variant

**Decision:**
```rust
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("Version mismatch on '{stream_id}': expected {expected}, got {actual}")]
    VersionMismatch {
        stream_id: String,
        expected: Version,
        actual: Version,
    },
}
```

**Rationale:** The kernel can only fail during rehydration (version sequence mismatch). All other errors (connection, serialization, conflict) belong in outer layers. Monolithic error enums are an anti-pattern for layered architecture.

### D9: `execute()` removed from `AggregateRoot`

**Decision:** `AggregateRoot` has no `execute()` method. No knowledge of commands, handlers, services, or async.

**Rationale:** Command dispatch is an application concern. The kernel aggregate is a synchronous state container with event tracking. Async, middleware, and dispatch patterns live in outer layers.

### D10: `Message` trait simplified

**Decision:**
```rust
pub trait Message: Send + Sync + Debug + 'static {}
```

**Removed:** `DowncastSync`, `Any`. These existed only to support runtime downcasting of trait objects, which is no longer needed with concrete event enums.

## Complete Kernel API

### Traits

```rust
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + 'static {}

pub trait Message: Send + Sync + Debug + 'static {}

pub trait DomainEvent: Message {
    fn name(&self) -> &'static str;
}

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
    fn name(&self) -> &'static str;
}

pub trait Aggregate {
    type State: AggregateState;
    type Error: std::error::Error + Send + Sync + Debug + 'static;
    type Id: Id;
}
```

### Type Aliases

```rust
pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;
```

### Value Types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    pub const INITIAL: Version = Version(0);
    pub fn next(self) -> Version { Version(self.0 + 1) }
    pub fn as_u64(self) -> u64 { self.0 }
}

impl Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Version {
    fn from(v: u64) -> Self { Version(v) }
}

#[derive(Debug)]
pub struct VersionedEvent<E> {
    pub version: Version,
    pub event: E,
}
```

### Error

```rust
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("Version mismatch on '{stream_id}': expected {expected}, got {actual}")]
    VersionMismatch {
        stream_id: String,
        expected: Version,
        actual: Version,
    },
}
```

### AggregateRoot

```rust
#[derive(Debug)]
pub struct AggregateRoot<A: Aggregate> {
    id: A::Id,
    state: A::State,
    version: Version,
    uncommitted_events: SmallVec<[VersionedEvent<EventOf<A>>; 1]>,
}

impl<A: Aggregate> AggregateRoot<A> {
    /// Create a new aggregate with default state at version 0.
    pub fn new(id: A::Id) -> Self;

    /// Access the aggregate's identity.
    pub fn id(&self) -> &A::Id;

    /// Access the current state (read-only).
    pub fn state(&self) -> &A::State;

    /// The last persisted version.
    pub fn version(&self) -> Version;

    /// Persisted version + uncommitted event count.
    pub fn current_version(&self) -> Version;

    /// Apply a single event: mutates state, increments version, tracks as uncommitted.
    pub fn apply_event(&mut self, event: EventOf<A>);

    /// Apply multiple events.
    pub fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<A>>);

    /// Rehydrate from persisted versioned events. Validates version sequence.
    pub fn load_from_events(
        id: A::Id,
        events: impl IntoIterator<Item = VersionedEvent<EventOf<A>>>,
    ) -> Result<Self, KernelError>;

    /// Drain uncommitted events for persistence.
    pub fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<A>>; 1]>;
}
```

### Events Collection (non-empty guarantee)

```rust
#[derive(Debug)]
pub struct Events<E: DomainEvent> {
    first: E,
    more: SmallVec<[E; 1]>,
}
```

**Change from current:** No `Box<E>` — events are stored by value since they are `Sized` enums.

## Kernel Dependencies

- `smallvec` — stack-allocated small vectors for event collections
- `thiserror` — derive macro for error types

**Removed:** `downcast-rs`, `async-trait`, `tokio-stream`, `tower`, `chrono`, `serde`, `uuid`

## What Moves OUT of the Kernel

| Concept | Destination |
|---|---|
| `Command` trait | Application layer |
| `Query` trait | Application layer |
| `AggregateCommandHandler` | Application layer |
| `CommandHandlerResponse` | Application layer |
| `HandlerFn` adapter | Application layer |
| `EventSourceRepository` | Application layer (port) |
| `ReadModel`, `ReadModelRepository` | Application layer (port) |
| `QueryHandlerFn` (tower) | Application layer |
| `EventStore`, `EventStreamer` | Port layer |
| `PendingEvent`, `PersistedEvent` | Event lifecycle layer |
| `PendingEventBuilder` | Event lifecycle layer |
| `EventMetadata` | Event lifecycle layer |
| `NexusId`, `EventId`, `CorrelationId` | Infrastructure layer |
| `Serializer`, `Deserializer` | Infrastructure layer |

## User Experience Summary

```rust
// 1. Event structs
#[derive(Debug, Clone)]
struct UserCreated { name: String }

#[derive(Debug, Clone)]
struct UserActivated;

// 2. Event enum (derive generates DomainEvent impl)
#[derive(DomainEvent, Debug, Clone)]
enum UserEvent {
    Created(UserCreated),
    Activated(UserActivated),
}

// 3. State
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

// 4. Aggregate marker
struct UserAggregate;

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = NexusId;
}

// 5. Business logic — directly on the aggregate
impl AggregateRoot<UserAggregate> {
    pub fn create(&mut self, name: String) -> Result<(), UserError> {
        if !self.state().name.is_empty() {
            return Err(UserError::AlreadyExists);
        }
        self.apply_event(UserEvent::Created(UserCreated { name }));
        Ok(())
    }

    pub fn activate(&mut self) -> Result<(), UserError> {
        if self.state().active {
            return Err(UserError::AlreadyActive);
        }
        self.apply_event(UserEvent::Activated(UserActivated));
        Ok(())
    }
}

// 6. Usage
let mut user = AggregateRoot::<UserAggregate>::new(id);
user.create("Alice".into())?;
user.activate()?;
let events = user.take_uncommitted_events(); // SmallVec<[VersionedEvent<UserEvent>; 1]>
```
