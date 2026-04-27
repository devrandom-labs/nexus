use crate::error::KernelError;
use crate::event::DomainEvent;
use crate::events::Events;
use crate::id::Id;
use crate::version::Version;
use std::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::num::NonZeroUsize;

/// State of an event-sourced aggregate. Mutated by applying domain events.
///
/// This is the **evolve** function: given a state and an event, produce
/// the next state. It is infallible — events are facts that have already
/// been accepted.
///
/// `Clone` is required so that [`AggregateRoot::replay`] can preserve the
/// original state if `apply` panics. The aggregate remains valid after
/// a panic in user code.
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
/// #[derive(Default, Debug, Clone)]
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
/// }
/// ```
pub trait AggregateState: Send + Sync + Debug + Clone + 'static {
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
    /// This method is infallible by design: events represent facts
    /// that have already been accepted. Validation happens in command
    /// handlers ([`Handle`]) before events are produced. If this method
    /// panics, it indicates a bug in the state machine.
    #[must_use]
    fn apply(self, event: &Self::Event) -> Self;
}

/// Type-level specification binding state, error, and ID types.
///
/// This is the marker trait — just associated types and configurable
/// constants, no methods. [`AggregateRoot<A>`] uses these types internally.
/// For command handling, see [`Handle`].
///
/// # Example
///
/// ```
/// use nexus::*;
/// use std::num::NonZeroUsize;
///
/// # #[derive(Debug, Clone)] enum Ev { A }
/// # impl Message for Ev {}
/// # impl DomainEvent for Ev { fn name(&self) -> &'static str { "A" } }
/// # #[derive(Default, Debug, Clone)] struct St;
/// # impl AggregateState for St { type Event = Ev; fn initial() -> Self { Self::default() } fn apply(self, _: &Ev) -> Self { self } }
/// # #[derive(Debug, Clone, Hash, PartialEq, Eq)] struct MyId(String);
/// # impl std::fmt::Display for MyId { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) } }
/// # impl AsRef<[u8]> for MyId { fn as_ref(&self) -> &[u8] { self.0.as_bytes() } }
/// # impl Id for MyId { const BYTE_LEN: usize = 0; }
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
/// ```
pub trait Aggregate: Sized {
    type State: AggregateState;
    type Error: Error + Send + Sync + Debug + 'static;
    type Id: Id;

    /// Maximum events during rehydration via [`AggregateRoot::replay`].
    /// Prevents a corrupted or malicious store from feeding unbounded events.
    /// Default: 1,000,000.
    const MAX_REHYDRATION_EVENTS: NonZeroUsize = DEFAULT_MAX_REHYDRATION_EVENTS;
}

/// Per-command handler trait — the **decide** function.
///
/// Implement this for each command your aggregate accepts. The handler
/// reads aggregate state via `&self`, validates invariants, and returns
/// decided events. It never mutates the aggregate — the repository
/// handles persistence and state advancement.
///
/// The const generic `N` controls how many *additional* events beyond
/// the first can be returned. Total capacity is `N + 1`. The default
/// `N = 0` means the handler returns exactly one event — the common
/// case for most commands. For multi-event handlers, specify `N`
/// explicitly (e.g., `Handle<CreateOrder, 2>` for up to 3 events).
///
/// # Example
///
/// Single-event handler (default `N = 0`):
///
/// ```
/// use nexus::*;
///
/// # #[derive(Debug, Clone)] enum TodoEvent { Created(String), Completed }
/// # impl Message for TodoEvent {}
/// # impl DomainEvent for TodoEvent { fn name(&self) -> &'static str { "e" } }
/// # #[derive(Default, Debug, Clone)] struct TodoState { done: bool }
/// # impl AggregateState for TodoState { type Event = TodoEvent; fn initial() -> Self { Self::default() } fn apply(self, _: &TodoEvent) -> Self { self } }
/// # #[derive(Debug, Clone, Hash, PartialEq, Eq)] struct TodoId(String);
/// # impl std::fmt::Display for TodoId { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) } }
/// # impl AsRef<[u8]> for TodoId { fn as_ref(&self) -> &[u8] { self.0.as_bytes() } }
/// # impl Id for TodoId { const BYTE_LEN: usize = 0; }
/// # #[derive(Debug, thiserror::Error)] #[error("e")] struct TodoError;
/// # struct Todo(AggregateRoot<Self>);
/// # impl Aggregate for Todo { type State = TodoState; type Error = TodoError; type Id = TodoId; }
/// # impl AggregateEntity for Todo { fn root(&self) -> &AggregateRoot<Self> { &self.0 } fn root_mut(&mut self) -> &mut AggregateRoot<Self> { &mut self.0 } }
///
/// struct CompleteTodo;
///
/// impl Handle<CompleteTodo> for Todo {
///     fn handle(&self, _cmd: CompleteTodo) -> Result<Events<TodoEvent>, TodoError> {
///         if self.state().done {
///             return Err(TodoError);
///         }
///         Ok(events![TodoEvent::Completed])
///     }
/// }
/// ```
pub trait Handle<C, const N: usize = 0>: AggregateEntity {
    /// Handle a command, returning decided events or a domain error.
    ///
    /// This is a pure decision — `&self` provides read-only access to
    /// the aggregate's state. External data (service results, enriched
    /// context) should be resolved by the application layer and passed
    /// as fields on the command `C`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the command violates a domain invariant.
    fn handle(&self, cmd: C) -> Result<Events<EventOf<Self>, N>, <Self as Aggregate>::Error>;
}

/// Trait for user-defined aggregate newtypes wrapping `AggregateRoot`.
///
/// Provides default method implementations so users get `state()`,
/// `version()`, etc. on their own type. Only `root()` and
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

    /// The last persisted version, or `None` for a fresh aggregate with no history.
    #[must_use]
    fn version(&self) -> Option<Version> {
        self.root().version()
    }

    /// Replay a single persisted event during rehydration.
    ///
    /// Delegates to [`AggregateRoot::replay`].
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] on version mismatch, overflow, or limit exceeded.
    fn replay(&mut self, version: Version, event: &EventOf<Self>) -> Result<(), KernelError> {
        self.root_mut().replay(version, event)
    }
}

/// Shorthand for accessing the event type of an aggregate.
///
/// Instead of writing `<<A as Aggregate>::State as AggregateState>::Event`,
/// write `EventOf<A>`.
pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;

/// Default maximum events during rehydration via `replay`.
/// Override per-aggregate via `Aggregate::MAX_REHYDRATION_EVENTS`.
#[allow(clippy::unwrap_used, reason = "1_000_000 is non-zero by inspection")]
pub const DEFAULT_MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(1_000_000).unwrap();

/// The core event-sourced aggregate container.
///
/// Holds state and version. The aggregate is a **read-only state container**
/// after loading — command handlers ([`Handle`]) read state to make decisions,
/// and the repository handles persistence and version advancement.
///
/// # Loading (rehydration)
///
/// The repository creates a new `AggregateRoot` and replays persisted events:
/// ```ignore
/// let mut root = AggregateRoot::<MyAggregate>::new(id);
/// for event in stored_events {
///     root.replay(event.version(), &event)?;
/// }
/// ```
///
/// # Command handling
///
/// After loading, the application layer calls [`Handle::handle`] to decide,
/// then persists the returned events via the repository. The aggregate
/// itself never buffers or persists events.
///
/// # Panic safety
///
/// If [`AggregateState::apply`] panics during [`replay`](Self::replay),
/// the aggregate remains fully valid — the original state is preserved
/// via clone and the version is not advanced.
pub struct AggregateRoot<A: Aggregate> {
    id: A::Id,
    state: A::State,
    version: Option<Version>,
}

impl<A: Aggregate> fmt::Debug for AggregateRoot<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AggregateRoot")
            .field("id", &self.id)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl<A: Aggregate> AggregateRoot<A> {
    /// Create a new aggregate with default state and no version history.
    pub fn new(id: A::Id) -> Self {
        Self {
            id,
            state: A::State::initial(),
            version: None,
        }
    }

    /// Create an aggregate root restored from a snapshot.
    ///
    /// The root is initialized with the given state and version,
    /// as if those events had already been replayed. Subsequent
    /// calls to [`replay`](Self::replay) will expect versions
    /// starting at `version + 1`.
    ///
    /// Used by snapshot-aware repositories to skip full event replay.
    #[must_use]
    pub const fn restore(id: A::Id, state: A::State, version: Version) -> Self {
        Self {
            id,
            state,
            version: Some(version),
        }
    }

    /// The aggregate's identity.
    #[must_use]
    pub const fn id(&self) -> &A::Id {
        &self.id
    }

    /// The current state (read-only). Used by [`Handle`] implementations
    /// to check invariants before producing events.
    #[must_use]
    pub const fn state(&self) -> &A::State {
        &self.state
    }

    /// The last persisted version, or `None` for a fresh aggregate with no history.
    #[must_use]
    pub const fn version(&self) -> Option<Version> {
        self.version
    }

    /// Replay a single persisted event during rehydration.
    ///
    /// Takes a borrowed event reference so zero-copy codecs (rkyv, flatbuffers)
    /// can pass views directly from database buffers without cloning.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::VersionMismatch`] if `version` is not the
    /// next expected version (gap, duplicate, or out-of-order).
    ///
    /// Returns [`KernelError::RehydrationLimitExceeded`] if `version` exceeds
    /// [`Aggregate::MAX_REHYDRATION_EVENTS`].
    ///
    /// Returns [`KernelError::VersionOverflow`] if the version sequence
    /// is exhausted (aggregate already at `u64::MAX`).
    ///
    /// # Panics
    ///
    /// Panics if `MAX_REHYDRATION_EVENTS` exceeds `u64::MAX` on the
    /// current platform (impossible on 32/64-bit systems).
    #[allow(
        clippy::expect_used,
        reason = "u64::try_from(usize) cannot fail on supported platforms (max 64-bit)"
    )]
    pub fn replay(&mut self, version: Version, event: &EventOf<A>) -> Result<(), KernelError> {
        let expected = match self.version {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(KernelError::VersionOverflow)?,
        };
        if version != expected {
            return Err(KernelError::VersionMismatch {
                expected,
                actual: version,
            });
        }
        if version.as_u64()
            > u64::try_from(A::MAX_REHYDRATION_EVENTS.get())
                .expect("MAX_REHYDRATION_EVENTS exceeds u64 on this platform")
        {
            return Err(KernelError::RehydrationLimitExceeded {
                max: A::MAX_REHYDRATION_EVENTS.get(),
            });
        }
        // Clone state before applying — if apply panics, original is preserved.
        let new_state = self.state.clone().apply(event);
        self.state = new_state;
        self.version = Some(version);
        Ok(())
    }

    /// Advance the version after successful persistence.
    ///
    /// Called by the repository after events have been written to the store.
    /// The version advances to reflect the newly persisted events.
    ///
    /// # Contract
    ///
    /// Only call after a successful write to the event store. Calling
    /// without persistence leaves the aggregate's version ahead of
    /// the store — subsequent saves will produce a version conflict.
    pub const fn advance_version(&mut self, new_version: Version) {
        self.version = Some(new_version);
    }

    /// Apply decided events to state without recording them.
    ///
    /// Called by the repository after successful persistence to keep
    /// the in-memory state in sync with the store. Events have already
    /// been persisted — this just updates the local projection.
    pub fn apply_events<const N: usize>(&mut self, events: &Events<EventOf<A>, N>) {
        for event in events {
            let new_state = self.state.clone().apply(event);
            self.state = new_state;
        }
    }

    /// Apply a single event to the aggregate state.
    ///
    /// Called by the repository after successful persistence to keep
    /// the in-memory state in sync. Clone-based for panic safety —
    /// if `apply` panics, the original state is preserved.
    pub fn apply_event(&mut self, event: &EventOf<A>) {
        let new_state = self.state.clone().apply(event);
        self.state = new_state;
    }
}
