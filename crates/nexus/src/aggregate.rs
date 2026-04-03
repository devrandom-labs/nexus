use crate::error::{ErrorId, KernelError};
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
///     fn initial() -> Self { Self::default() }
///     type Event = CounterEvent;
///     fn apply(mut self, event: &CounterEvent) -> Self {
///         match event {
///             CounterEvent::Incremented => self.value += 1,
///             CounterEvent::Decremented => self.value -= 1,
///         }
///         self
///     }
///     fn name(&self) -> &'static str { "Counter" }
/// }
/// ```
pub trait AggregateState: Send + Sync + Debug + 'static {
    type Event: DomainEvent;

    /// The initial state of a new aggregate.
    ///
    /// This replaces `Default` — use this when the zero-valued state
    /// is invalid for your domain. For simple cases, just return
    /// `Self { field: 0, ... }` or derive `Default` and call
    /// `Self::default()`.
    fn initial() -> Self;

    /// Apply a domain event, returning the new state.
    ///
    /// Takes `self` by value to guarantee atomic state transitions —
    /// either the entire function completes and returns a valid new
    /// state, or it panics and the old state is consumed. No partial
    /// mutation is possible.
    ///
    /// For simple cases, use `mut self` and return `self`:
    /// ```ignore
    /// fn apply(mut self, event: &MyEvent) -> Self {
    ///     self.field = new_value;
    ///     self
    /// }
    /// ```
    #[must_use]
    fn apply(self, event: &Self::Event) -> Self;
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
/// # impl AggregateState for St { type Event = Ev; fn initial() -> Self { Self::default() } fn apply(self, _: &Ev) -> Self { self } fn name(&self) -> &'static str { "S" } }
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
/// agg.apply(Ev::A);
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

    /// Maximum uncommitted events before `apply` panics.
    /// Override this for aggregates that produce many events per operation.
    /// Default: 1024.
    const MAX_UNCOMMITTED: usize = DEFAULT_MAX_UNCOMMITTED;

    /// Maximum events during rehydration via `replay`.
    /// Prevents a corrupted or malicious store from feeding unbounded events.
    /// Default: 1,000,000.
    const MAX_REHYDRATION_EVENTS: usize = DEFAULT_MAX_REHYDRATION_EVENTS;
}

/// Trait for user-defined aggregate newtypes wrapping `AggregateRoot`.
///
/// Provides default method implementations so users get `state()`,
/// `apply()`, etc. on their own type. Only `root()` and
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
    fn apply(&mut self, event: EventOf<Self>) {
        self.root_mut().apply(event);
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

/// Default maximum uncommitted events before `apply` panics.
/// Override per-aggregate via `Aggregate::MAX_UNCOMMITTED`.
pub const DEFAULT_MAX_UNCOMMITTED: usize = 1024;

/// Default maximum events during rehydration via `replay`.
/// Override per-aggregate via `Aggregate::MAX_REHYDRATION_EVENTS`.
pub const DEFAULT_MAX_REHYDRATION_EVENTS: usize = 1_000_000;

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
///     fn initial() -> Self { Self::default() }
///     fn apply(mut self, event: &TodoEvent) -> Self {
///         match event {
///             TodoEvent::Created(t) => self.title = t.clone(),
///             TodoEvent::Done => self.done = true,
///         }
///         self
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
/// todo.apply(TodoEvent::Created("Buy milk".into()));
/// todo.apply(TodoEvent::Done);
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

impl<A: Aggregate> Clone for AggregateRoot<A>
where
    A::Id: Clone,
    A::State: Clone,
    EventOf<A>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            state: self.state.clone(),
            version: self.version,
            uncommitted_events: self.uncommitted_events.clone(),
        }
    }
}

impl<A: Aggregate> PartialEq for AggregateRoot<A>
where
    A::Id: PartialEq,
    A::State: PartialEq,
    EventOf<A>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.state == other.state
            && self.version == other.version
            && self.uncommitted_events == other.uncommitted_events
    }
}

impl<A: Aggregate> AggregateRoot<A> {
    /// Create a new aggregate with default state at version 0.
    pub fn new(id: A::Id) -> Self {
        Self {
            id,
            state: A::State::initial(),
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
    #[allow(
        clippy::expect_used,
        reason = "critical safety invariant — overflow must crash, not silently wrap"
    )]
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

    /// Apply a single event: tracks as uncommitted, then transitions state.
    ///
    /// The event is recorded BEFORE the state transition. State is moved
    /// out via `mem::replace`, so `AggregateState::apply(self, &event)`
    /// either returns a fully-valid new state, or panics — in which case
    /// the state falls back to `initial()` (clean, known) and the event
    /// remains in `uncommitted_events` for recovery via replay.
    ///
    /// # Panics
    ///
    /// Panics if the number of uncommitted events reaches `Aggregate::MAX_UNCOMMITTED`.
    /// Call `take_uncommitted_events()` to drain before hitting the limit.
    #[allow(
        clippy::panic,
        reason = "critical safety invariants — exceeding limits must crash explicitly"
    )]
    pub fn apply(&mut self, event: EventOf<A>) {
        assert!(
            self.uncommitted_events.len() < A::MAX_UNCOMMITTED,
            "Uncommitted event limit reached ({limit}). {hint}",
            limit = A::MAX_UNCOMMITTED,
            hint = if A::MAX_UNCOMMITTED == 0 {
                "MAX_UNCOMMITTED is 0 — this aggregate cannot accept any events. \
                 Override Aggregate::MAX_UNCOMMITTED with a value > 0."
            } else {
                "Call take_uncommitted_events() to drain."
            },
        );
        let next_version = self.current_version().next();
        // Record the event FIRST — survives panics in state.apply()
        self.uncommitted_events
            .push(VersionedEvent::new(next_version, event));
        // Move state out (replaced with initial), apply atomically, put back.
        // If apply panics: state is initial() (clean), event is preserved.
        let old_state = std::mem::replace(&mut self.state, A::State::initial());
        let event_idx = self.uncommitted_events.len() - 1;
        self.state = old_state.apply(self.uncommitted_events[event_idx].event());
        debug_assert_eq!(
            self.current_version(),
            next_version,
            "Invariant violated: apply must increment current_version by exactly 1"
        );
    }

    /// Replay a single persisted event during rehydration.
    ///
    /// Takes a borrowed event reference so zero-copy codecs (rkyv, flatbuffers)
    /// can pass views directly from database buffers without cloning.
    ///
    /// Call this in a loop for each event read from the store:
    ///
    /// ```ignore
    /// let mut agg = AggregateRoot::<MyAggregate>::new(id);
    /// for ve in events {
    ///     let (version, event) = ve.into_parts();
    ///     agg.replay(version, &event)?;
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::VersionMismatch`] if `version` is not the
    /// next expected version (i.e., there is a gap, duplicate, or out-of-order event).
    ///
    /// Returns [`KernelError::RehydrationLimitExceeded`] if `version` exceeds
    /// `Aggregate::MAX_REHYDRATION_EVENTS`.
    ///
    /// # Panics
    ///
    /// Panics if `MAX_REHYDRATION_EVENTS` exceeds `u64::MAX` on the target
    /// platform (impossible on 32-bit and 64-bit systems).
    #[allow(
        clippy::expect_used,
        reason = "u64::try_from(usize) cannot fail on supported platforms (max 64-bit)"
    )]
    pub fn replay(&mut self, version: Version, event: &EventOf<A>) -> Result<(), KernelError> {
        let expected = self.version.next();
        if version != expected {
            return Err(KernelError::VersionMismatch {
                stream_id: ErrorId::from_display(&self.id),
                expected,
                actual: version,
            });
        }
        if version.as_u64()
            > u64::try_from(A::MAX_REHYDRATION_EVENTS)
                .expect("MAX_REHYDRATION_EVENTS exceeds u64 on this platform")
        {
            return Err(KernelError::RehydrationLimitExceeded {
                stream_id: ErrorId::from_display(&self.id),
                max: A::MAX_REHYDRATION_EVENTS,
            });
        }
        let old_state = std::mem::replace(&mut self.state, A::State::initial());
        self.state = old_state.apply(event);
        self.version = version;
        debug_assert!(
            self.uncommitted_events.is_empty(),
            "Invariant violated: replay must not produce uncommitted events"
        );
        Ok(())
    }

    /// Drain uncommitted events for persistence.
    ///
    /// Advances the persisted version to include the drained events.
    /// Subsequent `apply` calls will continue from the new version.
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

    /// Read-only access to uncommitted events.
    ///
    /// Used by the repository save path to encode events without draining.
    /// Call [`clear_uncommitted()`](Self::clear_uncommitted) after successful
    /// persistence to advance the version and clear the buffer.
    #[must_use]
    pub fn uncommitted_events(&self) -> &[VersionedEvent<EventOf<A>>] {
        &self.uncommitted_events
    }

    /// Advance the persisted version and clear the uncommitted buffer.
    ///
    /// Call this after events have been successfully persisted. The version
    /// advances to include all previously uncommitted events.
    ///
    /// # Contract
    ///
    /// Only call after a successful write to the event store. Calling
    /// without persistence leaves the aggregate's version ahead of
    /// the store — subsequent saves will conflict.
    pub fn clear_uncommitted(&mut self) {
        self.version = self.current_version();
        self.uncommitted_events.clear();
    }
}
