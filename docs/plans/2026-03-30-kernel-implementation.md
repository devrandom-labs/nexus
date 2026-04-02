# Kernel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite the core `nexus` crate as a pure kernel layer with maximum compile-time type safety, zero-cost event handling via concrete enums, and the marker-trait aggregate pattern.

**Architecture:** The kernel contains only domain primitives: traits (`Id`, `Message`, `DomainEvent`, `AggregateState`, `Aggregate`), value types (`Version`, `VersionedEvent`, `Events`), the `AggregateRoot<A>` struct, and `KernelError`. Everything else (commands, queries, handlers, stores, serialization, infrastructure IDs) moves to outer layers. The `#[derive(DomainEvent)]` macro is rewritten to work on enums instead of structs.

**Tech Stack:** Rust edition 2024, `smallvec`, `thiserror`, `syn`/`quote`/`proc-macro2` (macros only)

**Design doc:** `docs/plans/2026-03-30-kernel-design.md`

---

### Task 1: Create kernel module structure

**Files:**
- Create: `crates/nexus/src/kernel/mod.rs`
- Create: `crates/nexus/src/kernel/id.rs`
- Create: `crates/nexus/src/kernel/message.rs`
- Create: `crates/nexus/src/kernel/event.rs`
- Create: `crates/nexus/src/kernel/version.rs`
- Create: `crates/nexus/src/kernel/error.rs`
- Create: `crates/nexus/src/kernel/aggregate.rs`
- Create: `crates/nexus/src/kernel/events.rs`
- Modify: `crates/nexus/src/lib.rs`

**Step 1: Create the empty kernel module with submodules**

Create `crates/nexus/src/kernel/mod.rs`:
```rust
pub mod aggregate;
pub mod error;
pub mod event;
pub mod events;
pub mod id;
pub mod message;
pub mod version;
```

Add `pub mod kernel;` to `crates/nexus/src/lib.rs` (alongside existing modules — don't remove anything yet).

**Step 2: Verify it compiles**

Run: `cargo check -p nexus`
Expected: PASS (empty modules)

**Step 3: Commit**

```bash
git add crates/nexus/src/kernel/
git commit -m "feat(kernel): scaffold empty kernel module structure"
```

---

### Task 2: Implement `Version` newtype

**Files:**
- Modify: `crates/nexus/src/kernel/version.rs`
- Create: `crates/nexus/tests/kernel/mod.rs`
- Create: `crates/nexus/tests/kernel/version_tests.rs`

**Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel/mod.rs`:
```rust
mod version_tests;
```

Create `crates/nexus/tests/kernel/version_tests.rs`:
```rust
use nexus::kernel::version::Version;

#[test]
fn initial_version_is_zero() {
    assert_eq!(Version::INITIAL.as_u64(), 0);
}

#[test]
fn next_increments_by_one() {
    let v = Version::INITIAL;
    assert_eq!(v.next().as_u64(), 1);
    assert_eq!(v.next().next().as_u64(), 2);
}

#[test]
fn version_from_u64() {
    let v = Version::from(42u64);
    assert_eq!(v.as_u64(), 42);
}

#[test]
fn version_display() {
    let v = Version::from(7u64);
    assert_eq!(format!("{v}"), "7");
}

#[test]
fn version_ordering() {
    let v1 = Version::from(1u64);
    let v2 = Version::from(2u64);
    assert!(v1 < v2);
    assert!(v2 > v1);
}

#[test]
fn version_equality() {
    let v1 = Version::from(5u64);
    let v2 = Version::from(5u64);
    assert_eq!(v1, v2);
}

#[test]
fn version_copy_semantics() {
    let v1 = Version::from(3u64);
    let v2 = v1; // Copy
    assert_eq!(v1, v2); // v1 still usable
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus --test kernel`
Expected: FAIL — `Version` not found

**Step 3: Implement Version**

Write `crates/nexus/src/kernel/version.rs`:
```rust
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    pub const INITIAL: Version = Version(0);

    pub fn next(self) -> Version {
        Version(self.0 + 1)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Version {
    fn from(v: u64) -> Self {
        Version(v)
    }
}
```

Add to `crates/nexus/src/kernel/mod.rs` re-exports:
```rust
pub use version::Version;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus --test kernel`
Expected: PASS — all 7 tests

**Step 5: Commit**

```bash
git add crates/nexus/src/kernel/version.rs crates/nexus/tests/kernel/
git commit -m "feat(kernel): add Version newtype with INITIAL, next(), Display"
```

---

### Task 3: Implement `KernelError`

**Files:**
- Modify: `crates/nexus/src/kernel/error.rs`
- Create: `crates/nexus/tests/kernel/error_tests.rs`

**Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel/error_tests.rs`:
```rust
use nexus::kernel::error::KernelError;
use nexus::kernel::version::Version;

#[test]
fn version_mismatch_display() {
    let err = KernelError::VersionMismatch {
        stream_id: "user-123".to_string(),
        expected: Version::from(3u64),
        actual: Version::from(5u64),
    };
    let msg = format!("{err}");
    assert!(msg.contains("user-123"));
    assert!(msg.contains("3"));
    assert!(msg.contains("5"));
}

#[test]
fn kernel_error_is_std_error() {
    let err = KernelError::VersionMismatch {
        stream_id: "test".to_string(),
        expected: Version::INITIAL,
        actual: Version::from(1u64),
    };
    // Verify it implements std::error::Error
    let _: &dyn std::error::Error = &err;
}
```

Add `mod error_tests;` to `crates/nexus/tests/kernel/mod.rs`.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus --test kernel`
Expected: FAIL — `KernelError` not found

**Step 3: Implement KernelError**

Write `crates/nexus/src/kernel/error.rs`:
```rust
use super::version::Version;
use thiserror::Error;

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

Add `pub use error::KernelError;` to `kernel/mod.rs`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus --test kernel`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus/src/kernel/error.rs crates/nexus/tests/kernel/error_tests.rs
git commit -m "feat(kernel): add KernelError with VersionMismatch variant"
```

---

### Task 4: Implement kernel traits (`Id`, `Message`, `DomainEvent`, `AggregateState`, `Aggregate`)

**Files:**
- Modify: `crates/nexus/src/kernel/id.rs`
- Modify: `crates/nexus/src/kernel/message.rs`
- Modify: `crates/nexus/src/kernel/event.rs`
- Modify: `crates/nexus/src/kernel/aggregate.rs`
- Modify: `crates/nexus/src/kernel/mod.rs`
- Create: `crates/nexus/tests/kernel/traits_tests.rs`

**Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel/traits_tests.rs`:
```rust
use nexus::kernel::*;
use std::fmt;

// --- Test ID type ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Id for TestId {}

// --- Test Events ---
#[derive(Debug, Clone)]
struct ThingCreated { name: String }

#[derive(Debug, Clone)]
struct ThingActivated;

#[derive(Debug, Clone)]
enum ThingEvent {
    Created(ThingCreated),
    Activated(ThingActivated),
}

impl Message for ThingEvent {}

impl DomainEvent for ThingEvent {
    fn name(&self) -> &'static str {
        match self {
            ThingEvent::Created(_) => "ThingCreated",
            ThingEvent::Activated(_) => "ThingActivated",
        }
    }
}

// --- Test State ---
#[derive(Default, Debug)]
struct ThingState {
    name: String,
    active: bool,
}

impl AggregateState for ThingState {
    type Event = ThingEvent;

    fn apply(&mut self, event: &ThingEvent) {
        match event {
            ThingEvent::Created(e) => self.name = e.name.clone(),
            ThingEvent::Activated(_) => self.active = true,
        }
    }

    fn name(&self) -> &'static str {
        "Thing"
    }
}

// --- Test Aggregate ---
struct ThingAggregate;

impl Aggregate for ThingAggregate {
    type State = ThingState;
    type Error = ThingError;
    type Id = TestId;
}

#[derive(Debug, thiserror::Error)]
enum ThingError {
    #[error("already exists")]
    AlreadyExists,
}

// --- Verify EventOf alias ---
#[test]
fn event_of_resolves_correctly() {
    fn assert_event_type<A: Aggregate>()
    where
        EventOf<A>: DomainEvent,
    {}
    assert_event_type::<ThingAggregate>();
}

#[test]
fn domain_event_name_works() {
    let event = ThingEvent::Created(ThingCreated { name: "test".into() });
    assert_eq!(event.name(), "ThingCreated");

    let event = ThingEvent::Activated(ThingActivated);
    assert_eq!(event.name(), "ThingActivated");
}

#[test]
fn aggregate_state_apply_mutates() {
    let mut state = ThingState::default();
    state.apply(&ThingEvent::Created(ThingCreated { name: "hello".into() }));
    assert_eq!(state.name, "hello");
    assert!(!state.active);

    state.apply(&ThingEvent::Activated(ThingActivated));
    assert!(state.active);
}

#[test]
fn aggregate_state_name() {
    let state = ThingState::default();
    assert_eq!(state.name(), "Thing");
}
```

Add `mod traits_tests;` to `crates/nexus/tests/kernel/mod.rs`.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus --test kernel`
Expected: FAIL — traits not found

**Step 3: Implement the traits**

Write `crates/nexus/src/kernel/id.rs`:
```rust
use std::fmt::{Debug, Display};
use std::hash::Hash;

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + 'static {}
```

Write `crates/nexus/src/kernel/message.rs`:
```rust
use std::fmt::Debug;

pub trait Message: Send + Sync + Debug + 'static {}
```

Write `crates/nexus/src/kernel/event.rs`:
```rust
use super::message::Message;

pub trait DomainEvent: Message {
    fn name(&self) -> &'static str;
}
```

Write `crates/nexus/src/kernel/aggregate.rs`:
```rust
use super::event::DomainEvent;
use super::id::Id;
use std::error::Error;
use std::fmt::Debug;

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
    fn name(&self) -> &'static str;
}

pub trait Aggregate {
    type State: AggregateState;
    type Error: Error + Send + Sync + Debug + 'static;
    type Id: Id;
}

pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;
```

Update `crates/nexus/src/kernel/mod.rs`:
```rust
pub mod aggregate;
pub mod error;
pub mod event;
pub mod events;
pub mod id;
pub mod message;
pub mod version;

pub use aggregate::{Aggregate, AggregateState, EventOf};
pub use error::KernelError;
pub use event::DomainEvent;
pub use id::Id;
pub use message::Message;
pub use version::Version;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus --test kernel`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus/src/kernel/ crates/nexus/tests/kernel/
git commit -m "feat(kernel): add Id, Message, DomainEvent, AggregateState, Aggregate traits"
```

---

### Task 5: Implement `VersionedEvent` and `Events` collection

**Files:**
- Modify: `crates/nexus/src/kernel/version.rs` (add VersionedEvent)
- Modify: `crates/nexus/src/kernel/events.rs`
- Modify: `crates/nexus/src/kernel/mod.rs`
- Create: `crates/nexus/tests/kernel/events_tests.rs`

**Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel/events_tests.rs`:
```rust
use nexus::kernel::*;
use nexus::kernel::version::VersionedEvent;
use nexus::kernel::events::Events;

// Reuse ThingEvent from traits_tests (or redefine minimal version)
#[derive(Debug, Clone, PartialEq)]
struct Created;
#[derive(Debug, Clone, PartialEq)]
struct Activated;

#[derive(Debug, Clone, PartialEq)]
enum TestEvent { Created(Created), Activated(Activated) }
impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Created(_) => "Created",
            TestEvent::Activated(_) => "Activated",
        }
    }
}

#[test]
fn versioned_event_holds_version_and_event() {
    let ve = VersionedEvent {
        version: Version::from(1u64),
        event: TestEvent::Created(Created),
    };
    assert_eq!(ve.version, Version::from(1u64));
    assert_eq!(ve.event, TestEvent::Created(Created));
}

#[test]
fn events_guarantees_non_empty() {
    let events = Events::new(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
    assert!(!events.is_empty());
}

#[test]
fn events_add_increases_len() {
    let mut events = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    assert_eq!(events.len(), 2);
}

#[test]
fn events_into_iter() {
    let mut events = Events::new(TestEvent::Created(Created));
    events.add(TestEvent::Activated(Activated));
    let collected: Vec<_> = events.into_iter().collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0], TestEvent::Created(Created));
    assert_eq!(collected[1], TestEvent::Activated(Activated));
}

#[test]
fn events_from_single() {
    let events = Events::from(TestEvent::Created(Created));
    assert_eq!(events.len(), 1);
}
```

Add `mod events_tests;` to `crates/nexus/tests/kernel/mod.rs`.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus --test kernel`
Expected: FAIL

**Step 3: Implement VersionedEvent and Events**

Add to `crates/nexus/src/kernel/version.rs`:
```rust
#[derive(Debug)]
pub struct VersionedEvent<E> {
    pub version: Version,
    pub event: E,
}
```

Write `crates/nexus/src/kernel/events.rs`:
```rust
use super::event::DomainEvent;
use smallvec::{SmallVec, IntoIter as SmallVecIntoIter};
use std::iter::{Chain, Once, once};

#[derive(Debug)]
pub struct Events<E: DomainEvent> {
    first: E,
    more: SmallVec<[E; 1]>,
}

impl<E: DomainEvent> Events<E> {
    pub fn new(event: E) -> Self {
        Events {
            first: event,
            more: SmallVec::new(),
        }
    }

    pub fn add(&mut self, event: E) {
        self.more.push(event);
    }

    pub fn len(&self) -> usize {
        self.more.len() + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<E: DomainEvent> From<E> for Events<E> {
    fn from(event: E) -> Self {
        Events::new(event)
    }
}

impl<E: DomainEvent> IntoIterator for Events<E> {
    type Item = E;
    type IntoIter = Chain<Once<E>, SmallVecIntoIter<[E; 1]>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.first).chain(self.more)
    }
}

impl<'a, E: DomainEvent> IntoIterator for &'a Events<E> {
    type Item = &'a E;
    type IntoIter = Chain<Once<&'a E>, core::slice::Iter<'a, E>>;

    fn into_iter(self) -> Self::IntoIter {
        once(&self.first).chain(self.more.iter())
    }
}
```

Add to `kernel/mod.rs` re-exports:
```rust
pub use version::VersionedEvent;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus --test kernel`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus/src/kernel/ crates/nexus/tests/kernel/events_tests.rs
git commit -m "feat(kernel): add VersionedEvent and non-empty Events collection"
```

---

### Task 6: Implement `AggregateRoot<A>`

**Files:**
- Modify: `crates/nexus/src/kernel/aggregate.rs`
- Modify: `crates/nexus/src/kernel/mod.rs`
- Create: `crates/nexus/tests/kernel/aggregate_root_tests.rs`

**Step 1: Write the failing tests**

Create `crates/nexus/tests/kernel/aggregate_root_tests.rs`:
```rust
use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;
use nexus::kernel::version::VersionedEvent;
use std::fmt;

// --- Test domain (self-contained) ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}
impl Id for TestId {}

#[derive(Debug, Clone)]
struct ItemCreated { name: String }
#[derive(Debug, Clone)]
struct ItemDone;

#[derive(Debug, Clone)]
enum ItemEvent { Created(ItemCreated), Done(ItemDone) }
impl Message for ItemEvent {}
impl DomainEvent for ItemEvent {
    fn name(&self) -> &'static str {
        match self { ItemEvent::Created(_) => "ItemCreated", ItemEvent::Done(_) => "ItemDone" }
    }
}

#[derive(Default, Debug)]
struct ItemState { name: String, done: bool }
impl AggregateState for ItemState {
    type Event = ItemEvent;
    fn apply(&mut self, event: &ItemEvent) {
        match event {
            ItemEvent::Created(e) => self.name = e.name.clone(),
            ItemEvent::Done(_) => self.done = true,
        }
    }
    fn name(&self) -> &'static str { "Item" }
}

struct ItemAggregate;
#[derive(Debug, thiserror::Error)]
enum ItemError {
    #[error("already exists")]
    AlreadyExists,
    #[error("already done")]
    AlreadyDone,
}
impl Aggregate for ItemAggregate {
    type State = ItemState;
    type Error = ItemError;
    type Id = TestId;
}

// --- Business logic on aggregate ---
impl AggregateRoot<ItemAggregate> {
    pub fn create(&mut self, name: String) -> Result<(), ItemError> {
        if !self.state().name.is_empty() {
            return Err(ItemError::AlreadyExists);
        }
        self.apply_event(ItemEvent::Created(ItemCreated { name }));
        Ok(())
    }

    pub fn mark_done(&mut self) -> Result<(), ItemError> {
        if self.state().done {
            return Err(ItemError::AlreadyDone);
        }
        self.apply_event(ItemEvent::Done(ItemDone));
        Ok(())
    }
}

// --- Tests ---

#[test]
fn new_aggregate_has_initial_version() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    assert_eq!(agg.version(), Version::INITIAL);
    assert_eq!(agg.current_version(), Version::INITIAL);
}

#[test]
fn new_aggregate_has_default_state() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    assert_eq!(agg.state().name, "");
    assert!(!agg.state().done);
}

#[test]
fn apply_event_mutates_state_and_increments_version() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.apply_event(ItemEvent::Created(ItemCreated { name: "test".into() }));
    assert_eq!(agg.state().name, "test");
    assert_eq!(agg.current_version(), Version::from(1u64));
    assert_eq!(agg.version(), Version::INITIAL); // persisted version unchanged
}

#[test]
fn take_uncommitted_events_drains() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.apply_event(ItemEvent::Created(ItemCreated { name: "test".into() }));
    agg.apply_event(ItemEvent::Done(ItemDone));

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].version, Version::from(1u64));
    assert_eq!(events[1].version, Version::from(2u64));

    // After drain, no uncommitted events
    let events2 = agg.take_uncommitted_events();
    assert!(events2.is_empty());
}

#[test]
fn load_from_events_rehydrates() {
    let events = vec![
        VersionedEvent { version: Version::from(1u64), event: ItemEvent::Created(ItemCreated { name: "loaded".into() }) },
        VersionedEvent { version: Version::from(2u64), event: ItemEvent::Done(ItemDone) },
    ];
    let agg = AggregateRoot::<ItemAggregate>::load_from_events(TestId("1".into()), events).unwrap();
    assert_eq!(agg.version(), Version::from(2u64));
    assert_eq!(agg.state().name, "loaded");
    assert!(agg.state().done);
}

#[test]
fn load_from_events_rejects_version_gap() {
    let events = vec![
        VersionedEvent { version: Version::from(1u64), event: ItemEvent::Created(ItemCreated { name: "test".into() }) },
        VersionedEvent { version: Version::from(3u64), event: ItemEvent::Done(ItemDone) }, // gap!
    ];
    let result = AggregateRoot::<ItemAggregate>::load_from_events(TestId("1".into()), events);
    assert!(result.is_err());
    match result.unwrap_err() {
        KernelError::VersionMismatch { expected, actual, .. } => {
            assert_eq!(expected, Version::from(2u64));
            assert_eq!(actual, Version::from(3u64));
        }
    }
}

#[test]
fn business_logic_on_aggregate_works() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.create("My Item".into()).unwrap();
    assert_eq!(agg.state().name, "My Item");

    agg.mark_done().unwrap();
    assert!(agg.state().done);

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 2);
}

#[test]
fn business_logic_enforces_invariants() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.create("Item".into()).unwrap();

    // Cannot create again
    let err = agg.create("Another".into()).unwrap_err();
    assert!(matches!(err, ItemError::AlreadyExists));
}

#[test]
fn aggregate_id_accessible() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("abc".into()));
    assert_eq!(agg.id(), &TestId("abc".into()));
}
```

Add `mod aggregate_root_tests;` to `crates/nexus/tests/kernel/mod.rs`.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p nexus --test kernel`
Expected: FAIL — `AggregateRoot` not found

**Step 3: Implement AggregateRoot**

Add to `crates/nexus/src/kernel/aggregate.rs`:
```rust
use super::error::KernelError;
use super::version::{Version, VersionedEvent};
use smallvec::{SmallVec, smallvec};

// ... existing trait definitions ...

#[derive(Debug)]
pub struct AggregateRoot<A: Aggregate> {
    id: A::Id,
    state: A::State,
    version: Version,
    uncommitted_events: SmallVec<[VersionedEvent<EventOf<A>>; 1]>,
}

impl<A: Aggregate> AggregateRoot<A> {
    pub fn new(id: A::Id) -> Self {
        Self {
            id,
            state: A::State::default(),
            version: Version::INITIAL,
            uncommitted_events: smallvec![],
        }
    }

    pub fn id(&self) -> &A::Id {
        &self.id
    }

    pub fn state(&self) -> &A::State {
        &self.state
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn current_version(&self) -> Version {
        let uncommitted_count = self.uncommitted_events.len() as u64;
        Version::from(self.version.as_u64() + uncommitted_count)
    }

    pub fn apply_event(&mut self, event: EventOf<A>) {
        self.state.apply(&event);
        let version = self.current_version().next();
        self.uncommitted_events.push(VersionedEvent { version, event });
    }

    pub fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<A>>) {
        for event in events {
            self.apply_event(event);
        }
    }

    pub fn load_from_events(
        id: A::Id,
        events: impl IntoIterator<Item = VersionedEvent<EventOf<A>>>,
    ) -> Result<Self, KernelError> {
        let mut aggregate = Self::new(id);
        for versioned_event in events {
            let expected = aggregate.version.next();
            if versioned_event.version != expected {
                return Err(KernelError::VersionMismatch {
                    stream_id: aggregate.id.to_string(),
                    expected,
                    actual: versioned_event.version,
                });
            }
            aggregate.state.apply(&versioned_event.event);
            aggregate.version = versioned_event.version;
        }
        Ok(aggregate)
    }

    pub fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<A>>; 1]> {
        std::mem::take(&mut self.uncommitted_events)
    }
}
```

Add `pub use aggregate::AggregateRoot;` to `kernel/mod.rs`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p nexus --test kernel`
Expected: PASS — all tests including business logic on `AggregateRoot<ItemAggregate>`

**Step 5: Run clippy**

Run: `cargo clippy -p nexus -- --deny warnings`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/nexus/src/kernel/ crates/nexus/tests/kernel/
git commit -m "feat(kernel): add AggregateRoot<A> with apply_event, load_from_events, take_uncommitted_events"
```

---

### Task 7: Rewrite `#[derive(DomainEvent)]` macro for enums

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs`
- Create: `crates/nexus-macros/tests/domain_event_enum.rs`

**Step 1: Write the failing test**

Create `crates/nexus-macros/tests/domain_event_enum.rs`:
```rust
use nexus::kernel::{Message, DomainEvent};

#[derive(Debug, Clone, nexus::DomainEvent)]
enum TestEvent {
    Created(Created),
    Updated(Updated),
    Deleted,
}

#[derive(Debug, Clone)]
struct Created { name: String }

#[derive(Debug, Clone)]
struct Updated { name: String }

#[test]
fn derive_domain_event_on_enum_generates_name() {
    let e = TestEvent::Created(Created { name: "test".into() });
    assert_eq!(e.name(), "Created");

    let e = TestEvent::Updated(Updated { name: "new".into() });
    assert_eq!(e.name(), "Updated");

    let e = TestEvent::Deleted;
    assert_eq!(e.name(), "Deleted");
}

#[test]
fn derive_domain_event_implements_message() {
    fn assert_message<T: Message>() {}
    assert_message::<TestEvent>();
}

#[test]
fn derive_domain_event_implements_domain_event() {
    fn assert_domain_event<T: DomainEvent>() {}
    assert_domain_event::<TestEvent>();
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-macros --test domain_event_enum`
Expected: FAIL — current macro rejects enums with "Enums are not supported"

**Step 3: Rewrite the DomainEvent derive macro**

In `crates/nexus-macros/src/lib.rs`, replace `parse_domain_event`:

```rust
fn parse_domain_event(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    match &ast.data {
        Data::Enum(data_enum) => {
            let variant_arms: Vec<_> = data_enum
                .variants
                .iter()
                .map(|variant| {
                    let variant_ident = &variant.ident;
                    let variant_name = variant_ident.to_string();
                    // Handle all variant shapes: unit, tuple, struct
                    match &variant.fields {
                        syn::Fields::Unit => {
                            quote! { #name::#variant_ident => #variant_name }
                        }
                        syn::Fields::Unnamed(_) => {
                            quote! { #name::#variant_ident(..) => #variant_name }
                        }
                        syn::Fields::Named(_) => {
                            quote! { #name::#variant_ident { .. } => #variant_name }
                        }
                    }
                })
                .collect();

            let expanded = quote! {
                impl ::nexus::kernel::Message for #name {}

                impl ::nexus::kernel::DomainEvent for #name {
                    fn name(&self) -> &'static str {
                        match self {
                            #(#variant_arms),*
                        }
                    }
                }
            };

            Ok(expanded)
        }
        Data::Struct(_) => Err(Error::new(
            name.span(),
            "DomainEvent derive now requires an enum. Wrap event structs in an enum: `enum MyEvent { Created(Created), ... }`",
        )),
        Data::Union(_) => Err(Error::new(name.span(), "Unions are not supported.")),
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-macros --test domain_event_enum`
Expected: PASS

**Step 5: Verify the old struct-based derive now gives a helpful error**

Manually verify (or add a trybuild test in a later task) that `#[derive(DomainEvent)]` on a struct produces the error message about needing an enum.

**Step 6: Commit**

```bash
git add crates/nexus-macros/src/lib.rs crates/nexus-macros/tests/
git commit -m "feat(macros): rewrite DomainEvent derive to work on enums with per-variant name()"
```

---

### Task 8: Full integration test — complete user experience

**Files:**
- Create: `crates/nexus/tests/kernel/integration_test.rs`

**Step 1: Write the integration test**

This test exercises the complete user experience from the design doc.

Create `crates/nexus/tests/kernel/integration_test.rs`:
```rust
//! Integration test: the complete kernel user experience.
//! This mirrors the "User Experience Summary" from the kernel design doc.

use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;
use std::fmt;

// --- ID ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UserId(u64);
impl fmt::Display for UserId { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "user-{}", self.0) } }
impl Id for UserId {}

// --- Events ---
#[derive(Debug, Clone)]
struct UserCreated { name: String }

#[derive(Debug, Clone)]
struct UserActivated;

#[derive(Debug, Clone, nexus::DomainEvent)]
enum UserEvent {
    Created(UserCreated),
    Activated(UserActivated),
}

// --- State ---
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

// --- Aggregate ---
struct UserAggregate;

#[derive(Debug, thiserror::Error)]
enum UserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user already active")]
    AlreadyActive,
}

impl Aggregate for UserAggregate {
    type State = UserState;
    type Error = UserError;
    type Id = UserId;
}

// --- Business Logic ---
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

// --- Tests ---

#[test]
fn full_aggregate_lifecycle() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId(1));

    // Execute business operations
    user.create("Alice".into()).unwrap();
    user.activate().unwrap();

    // Verify state
    assert_eq!(user.state().name, "Alice");
    assert!(user.state().active);
    assert_eq!(user.current_version(), Version::from(2u64));

    // Take events for persistence
    let events = user.take_uncommitted_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].version, Version::from(1u64));
    assert_eq!(events[1].version, Version::from(2u64));
    assert_eq!(events[0].event.name(), "Created");
    assert_eq!(events[1].event.name(), "Activated");
}

#[test]
fn rehydrate_then_continue() {
    use nexus::kernel::version::VersionedEvent;

    // Simulate loading from event store
    let history = vec![
        VersionedEvent { version: Version::from(1u64), event: UserEvent::Created(UserCreated { name: "Bob".into() }) },
    ];
    let mut user = AggregateRoot::<UserAggregate>::load_from_events(UserId(2), history).unwrap();

    assert_eq!(user.version(), Version::from(1u64));
    assert_eq!(user.state().name, "Bob");

    // Continue with new operations
    user.activate().unwrap();
    let events = user.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].version, Version::from(2u64));
}

#[test]
fn invariant_violations_return_domain_errors() {
    let mut user = AggregateRoot::<UserAggregate>::new(UserId(3));
    user.create("Charlie".into()).unwrap();

    // Business rule: can't create twice
    assert!(matches!(
        user.create("David".into()),
        Err(UserError::AlreadyExists)
    ));

    // Business rule: can't activate twice
    user.activate().unwrap();
    assert!(matches!(
        user.activate(),
        Err(UserError::AlreadyActive)
    ));
}
```

Add `mod integration_test;` to `crates/nexus/tests/kernel/mod.rs`.

**Step 2: Run the integration test**

Run: `cargo test -p nexus --test kernel -- integration_test`
Expected: PASS

**Step 3: Run full test suite and clippy**

Run: `cargo test -p nexus --test kernel && cargo clippy -p nexus -- --deny warnings && cargo fmt --all --check`
Expected: ALL PASS

**Step 4: Commit**

```bash
git add crates/nexus/tests/kernel/integration_test.rs
git commit -m "test(kernel): add full integration test for kernel user experience"
```

---

### Task 9: Add `events![]` macro to kernel

**Files:**
- Modify: `crates/nexus/src/kernel/events.rs`
- Add tests to: `crates/nexus/tests/kernel/events_tests.rs`

**Step 1: Write the failing test**

Add to `crates/nexus/tests/kernel/events_tests.rs`:
```rust
#[test]
fn events_macro_single() {
    let events = nexus::events![TestEvent::Created(Created)];
    assert_eq!(events.len(), 1);
}

#[test]
fn events_macro_multiple() {
    let events = nexus::events![
        TestEvent::Created(Created),
        TestEvent::Activated(Activated),
    ];
    assert_eq!(events.len(), 2);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus --test kernel -- events_macro`
Expected: FAIL — macro not found

**Step 3: Add events! macro**

Add to `crates/nexus/src/kernel/events.rs`:
```rust
#[macro_export]
macro_rules! events {
    [$head:expr] => {
        $crate::kernel::events::Events::new($head)
    };
    [$head:expr, $($tail:expr),+ $(,)?] => {
        {
            let mut events = $crate::kernel::events::Events::new($head);
            $(
                events.add($tail);
            )*
            events
        }
    };
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus --test kernel -- events_macro`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus/src/kernel/events.rs crates/nexus/tests/kernel/events_tests.rs
git commit -m "feat(kernel): add events![] macro for non-empty event collections"
```

---

### Summary

| Task | What | Tests |
|------|------|-------|
| 1 | Module structure scaffold | Compiles |
| 2 | `Version` newtype | 7 tests |
| 3 | `KernelError` | 2 tests |
| 4 | Traits (`Id`, `Message`, `DomainEvent`, `AggregateState`, `Aggregate`) | 4 tests |
| 5 | `VersionedEvent`, `Events` collection | 5 tests |
| 6 | `AggregateRoot<A>` | 9 tests |
| 7 | Rewrite `#[derive(DomainEvent)]` for enums | 3 tests |
| 8 | Full integration test | 3 tests |
| 9 | `events![]` macro | 2 tests |

**Total: 9 tasks, 35 tests, ~9 commits**

**Note:** The old `domain/`, `command/`, `query/`, `event/`, `infra/`, `store/`, `serde/` modules remain untouched. They coexist with the new `kernel/` module. A follow-up plan will migrate the outer layers to use the kernel and eventually remove the old modules.
