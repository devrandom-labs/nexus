use crate::error::KernelError;
use crate::event::DomainEvent;
use crate::id::Id;
use crate::version::{Version, VersionedEvent};
use smallvec::{SmallVec, smallvec};
use std::error::Error;
use std::fmt::Debug;

/// State of an event-sourced aggregate. Mutated by applying domain events.
///
/// Implement this on your state struct. The `Event` associated type binds
/// this state to its event enum — the compiler enforces that only matching
/// events can be applied.
///
/// # Example
///
/// ```
/// use nexus::AggregateState;
/// use nexus::DomainEvent;
/// use nexus::Message;
///
/// #[derive(Debug, Clone)]
/// enum CounterEvent { Incremented, Decremented }
/// impl Message for CounterEvent {}
/// impl DomainEvent for CounterEvent {
///     fn name(&self) -> &'static str {
///         match self {
///             CounterEvent::Incremented => "Incremented",
///             CounterEvent::Decremented => "Decremented",
///         }
///     }
/// }
///
/// #[derive(Default, Debug)]
/// struct CounterState { value: i64 }
///
/// impl AggregateState for CounterState {
///     type Event = CounterEvent;
///     fn apply(&mut self, event: &CounterEvent) {
///         match event {
///             CounterEvent::Incremented => self.value += 1,
///             CounterEvent::Decremented => self.value -= 1,
///         }
///     }
///     fn name(&self) -> &'static str { "Counter" }
/// }
/// ```
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
    fn name(&self) -> &'static str;
}

/// The aggregate trait — binds state, error, and ID types together
/// and provides default method implementations via `root()`/`root_mut()`.
///
/// Users implement this on a newtype wrapping `AggregateRoot<Self>`.
/// The `#[derive(Aggregate)]` macro generates this boilerplate.
/// Only `root()` and `root_mut()` need implementing — all other
/// methods are provided as defaults.
///
/// # Manual Example (what the derive macro generates)
///
/// ```
/// use nexus::*;
///
/// # #[derive(Debug, Clone)] enum Ev { A }
/// # impl Message for Ev {}
/// # impl DomainEvent for Ev { fn name(&self) -> &'static str { "A" } }
/// # #[derive(Default, Debug)] struct St;
/// # impl AggregateState for St { type Event = Ev; fn apply(&mut self, _: &Ev) {} fn name(&self) -> &'static str { "S" } }
/// # #[derive(Debug, Clone, Hash, PartialEq, Eq)] struct MyId(u64);
/// # impl std::fmt::Display for MyId { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) } }
/// # impl Id for MyId {}
/// # #[derive(Debug, thiserror::Error)] #[error("e")] struct MyError;
///
/// struct MyAggregate(AggregateRoot<Self>);
///
/// impl Aggregate for MyAggregate {
///     type State = St;
///     type Error = MyError;
///     type Id = MyId;
/// }
///
/// impl AggregateEntity for MyAggregate {
///     fn root(&self) -> &AggregateRoot<Self> { &self.0 }
///     fn root_mut(&mut self) -> &mut AggregateRoot<Self> { &mut self.0 }
/// }
///
/// impl MyAggregate {
///     fn new(id: MyId) -> Self { Self(AggregateRoot::new(id)) }
/// }
///
/// let mut agg = MyAggregate::new(MyId(1));
/// agg.apply_event(Ev::A);
/// assert_eq!(agg.version(), Version::INITIAL);
/// assert_eq!(agg.current_version(), Version::from_persisted(1));
/// ```
/// Type-level specification binding state, error, and ID types.
///
/// This is the marker trait — just associated types, no methods.
/// `AggregateRoot<A: Aggregate>` uses these types internally.
pub trait Aggregate: Sized {
    type State: AggregateState;
    type Error: Error + Send + Sync + Debug + 'static;
    type Id: Id;

    /// Maximum uncommitted events before `apply_event` panics.
    /// Override this for aggregates that produce many events per operation.
    /// Default: 1024.
    const MAX_UNCOMMITTED: usize = DEFAULT_MAX_UNCOMMITTED;
}

/// Trait for user-defined aggregate newtypes wrapping `AggregateRoot`.
///
/// Provides default method implementations so users get `state()`,
/// `apply_event()`, etc. on their own type. Only `root()` and
/// `root_mut()` must be implemented — typically via `#[derive(Aggregate)]`.
pub trait AggregateEntity: Aggregate {
    /// Access the inner `AggregateRoot`.
    fn root(&self) -> &AggregateRoot<Self>;

    /// Mutably access the inner `AggregateRoot`.
    fn root_mut(&mut self) -> &mut AggregateRoot<Self>;

    /// The aggregate's identity.
    #[must_use]
    fn id(&self) -> &Self::Id {
        self.root().id()
    }

    /// The current state (read-only).
    #[must_use]
    fn state(&self) -> &Self::State {
        self.root().state()
    }

    /// The last persisted version.
    #[must_use]
    fn version(&self) -> Version {
        self.root().version()
    }

    /// Persisted version + uncommitted event count.
    #[must_use]
    fn current_version(&self) -> Version {
        self.root().current_version()
    }

    /// Apply a single event.
    fn apply_event(&mut self, event: EventOf<Self>) {
        self.root_mut().apply_event(event);
    }

    /// Apply multiple events.
    fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<Self>>) {
        self.root_mut().apply_events(events);
    }

    /// Drain uncommitted events for persistence.
    fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<Self>>; 1]> {
        self.root_mut().take_uncommitted_events()
    }
}

/// Shorthand for accessing the event type of an aggregate.
///
/// Instead of writing `<<A as Aggregate>::State as AggregateState>::Event`,
/// write `EventOf<A>`.
pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;

/// Default maximum uncommitted events before `apply_event` panics.
/// Override per-aggregate via `Aggregate::MAX_UNCOMMITTED`.
pub const DEFAULT_MAX_UNCOMMITTED: usize = 1024;

/// The core event-sourced aggregate container.
///
/// Holds state, tracks version, and collects uncommitted events.
/// The maximum number of uncommitted events is bounded by
/// `Aggregate::MAX_UNCOMMITTED` (default: 1024) to prevent
/// unbounded memory growth on constrained devices.
///
/// # Example
///
/// ```
/// use nexus::{Aggregate, AggregateRoot, AggregateState};
/// use nexus::DomainEvent;
/// use nexus::Id;
/// use nexus::Message;
/// use nexus::Version;
///
/// // Define event, state, aggregate (minimal)
/// #[derive(Debug, Clone)]
/// enum TodoEvent { Created(String), Done }
/// impl Message for TodoEvent {}
/// impl DomainEvent for TodoEvent {
///     fn name(&self) -> &'static str {
///         match self { TodoEvent::Created(_) => "Created", TodoEvent::Done => "Done" }
///     }
/// }
///
/// #[derive(Default, Debug)]
/// struct TodoState { title: String, done: bool }
/// impl AggregateState for TodoState {
///     type Event = TodoEvent;
///     fn apply(&mut self, event: &TodoEvent) {
///         match event {
///             TodoEvent::Created(t) => self.title = t.clone(),
///             TodoEvent::Done => self.done = true,
///         }
///     }
///     fn name(&self) -> &'static str { "Todo" }
/// }
///
/// #[derive(Debug, Clone, Hash, PartialEq, Eq)]
/// struct TodoId(u64);
/// impl std::fmt::Display for TodoId {
///     fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) }
/// }
/// impl Id for TodoId {}
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("todo error")]
/// struct TodoError;
///
/// struct TodoAggregate;
/// impl Aggregate for TodoAggregate {
///     type State = TodoState;
///     type Error = TodoError;
///     type Id = TodoId;
/// }
///
/// // Use it
/// let mut todo = AggregateRoot::<TodoAggregate>::new(TodoId(1));
/// todo.apply_event(TodoEvent::Created("Buy milk".into()));
/// todo.apply_event(TodoEvent::Done);
///
/// assert_eq!(todo.state().title, "Buy milk");
/// assert!(todo.state().done);
/// assert_eq!(todo.current_version(), Version::from_persisted(2));
///
/// let events = todo.take_uncommitted_events();
/// assert_eq!(events.len(), 2);
/// ```
#[derive(Debug)]
pub struct AggregateRoot<A: Aggregate> {
    id: A::Id,
    state: A::State,
    version: Version,
    uncommitted_events: SmallVec<[VersionedEvent<EventOf<A>>; 1]>,
}

impl<A: Aggregate> AggregateRoot<A> {
    /// Create a new aggregate with default state at version 0.
    pub fn new(id: A::Id) -> Self {
        Self {
            id,
            state: A::State::default(),
            version: Version::INITIAL,
            uncommitted_events: smallvec![],
        }
    }

    /// The aggregate's identity.
    #[must_use]
    pub const fn id(&self) -> &A::Id {
        &self.id
    }

    /// The current state (read-only). Use this in business logic
    /// methods to check invariants before producing events.
    #[must_use]
    pub const fn state(&self) -> &A::State {
        &self.state
    }

    /// The last persisted version. Does not include uncommitted events.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// Persisted version + uncommitted event count.
    ///
    /// # Panics
    ///
    /// Panics if the total version count overflows `u64`.
    #[must_use]
    pub fn current_version(&self) -> Version {
        let uncommitted_count = u64::try_from(self.uncommitted_events.len())
            .expect("uncommitted event count exceeds u64");
        let total = self
            .version
            .as_u64()
            .checked_add(uncommitted_count)
            .expect("Version overflow: version + uncommitted count exceeds u64::MAX");
        Version::new(total)
    }

    /// Apply a single event: mutates state, increments version, tracks as uncommitted.
    ///
    /// # Panics
    ///
    /// Panics if the number of uncommitted events reaches `Aggregate::MAX_UNCOMMITTED`.
    /// Call `take_uncommitted_events()` to drain before hitting the limit.
    pub fn apply_event(&mut self, event: EventOf<A>) {
        assert!(
            self.uncommitted_events.len() < A::MAX_UNCOMMITTED,
            "Uncommitted event limit reached ({}). Call take_uncommitted_events() to drain.",
            A::MAX_UNCOMMITTED,
        );
        let next_version = self.current_version().next();
        self.state.apply(&event);
        self.uncommitted_events
            .push(VersionedEvent::new(next_version, event));
        debug_assert_eq!(
            self.current_version(),
            next_version,
            "Invariant violated: apply_event must increment current_version by exactly 1"
        );
    }

    /// Apply multiple events. Pre-allocates if the iterator provides a size hint.
    pub fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<A>>) {
        let iter = events.into_iter();
        let (lower, _) = iter.size_hint();
        self.uncommitted_events.reserve(lower);
        for event in iter {
            self.apply_event(event);
        }
    }

    /// Rehydrate an aggregate from persisted versioned events.
    ///
    /// Validates that version numbers are strictly sequential starting from 1.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::VersionMismatch`] if any event version is not
    /// strictly sequential (i.e., there is a gap or duplicate).
    pub fn load_from_events(
        id: A::Id,
        events: impl IntoIterator<Item = VersionedEvent<EventOf<A>>>,
    ) -> Result<Self, KernelError> {
        let mut aggregate = Self::new(id);
        for versioned_event in events {
            let expected = aggregate.version.next();
            let (version, event) = versioned_event.into_parts();
            if version != expected {
                return Err(KernelError::VersionMismatch {
                    stream_id: aggregate.id.to_string(),
                    expected,
                    actual: version,
                });
            }
            aggregate.state.apply(&event);
            aggregate.version = version;
        }
        debug_assert!(
            aggregate.uncommitted_events.is_empty(),
            "Invariant violated: load_from_events must not produce uncommitted events"
        );
        Ok(aggregate)
    }

    /// Drain uncommitted events for persistence.
    ///
    /// Advances the persisted version to include the drained events.
    /// Subsequent `apply_event` calls will continue from the new version.
    pub fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<A>>; 1]> {
        self.version = self.current_version();
        let events = std::mem::take(&mut self.uncommitted_events);
        debug_assert_eq!(
            self.version,
            self.current_version(),
            "Invariant violated: after take, version must equal current_version"
        );
        debug_assert!(
            self.uncommitted_events.is_empty(),
            "Invariant violated: after take, uncommitted_events must be empty"
        );
        events
    }
}
