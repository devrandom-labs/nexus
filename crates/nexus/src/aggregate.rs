use crate::error::KernelError;
use crate::event::DomainEvent;
use crate::events::Events;
use crate::id::Id;
use crate::version::Version;
use std::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::mem;
use std::num::NonZeroUsize;

/// State of an event-sourced aggregate. Mutated by applying domain events.
///
/// This is the **evolve** function: given a state and an event, produce
/// the next state. It is infallible — events are facts that have already
/// been accepted.
///
/// No `Clone` bound: [`apply`](Self::apply) takes `self` by value and
/// returns the next state, so rehydration folds the owned state through
/// `apply` with no per-event copy.
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
/// struct MyAggregate;
///
/// impl Aggregate for MyAggregate {
///     type State = St;
///     type Error = MyError;
///     type Id = MyId;
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
/// Implement this on the aggregate marker for each command it accepts. The
/// handler reads the current `state`, validates invariants, and returns
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
/// # struct Todo;
/// # impl Aggregate for Todo { type State = TodoState; type Error = TodoError; type Id = TodoId; }
///
/// struct CompleteTodo;
///
/// impl Handle<CompleteTodo> for Todo {
///     fn handle(state: &TodoState, _cmd: CompleteTodo) -> Result<Events<TodoEvent>, TodoError> {
///         if state.done {
///             return Err(TodoError);
///         }
///         Ok(events![TodoEvent::Completed])
///     }
/// }
/// ```
pub trait Handle<C, const N: usize = 0>: Aggregate {
    /// Decide a command, returning decided events or a domain error.
    ///
    /// A **pure decision**: reads the aggregate's current `state` and the
    /// command, returns the decided events. No access to version or identity —
    /// a decision is a function of domain state and command only, never of
    /// persistence position. Implemented on the aggregate marker type; invoke
    /// via [`AggregateRoot::handle`] on a loaded aggregate.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the command violates a domain invariant.
    fn handle(state: &Self::State, cmd: C) -> Result<Events<EventOf<Self>, N>, Self::Error>;
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
/// itself never buffers or persists events. After a durable persist the
/// driver calls [`commit_persisted`](Self::commit_persisted) once to advance
/// the version and fold the events into state atomically.
///
/// # Panic safety
///
/// [`replay`](Self::replay) and [`commit_persisted`](Self::commit_persisted)
/// move the state out via [`mem::replace`] and fold it through
/// [`AggregateState::apply`] — no clone. If `apply` panics (a state-machine
/// bug), the state is left at [`AggregateState::initial`]: valid, never
/// partially mutated.
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

    /// Decide a command against the current state.
    ///
    /// Dispatches to the aggregate's [`Handle`] impl, passing the current
    /// [`state`](Self::state). Pure — reads state, returns decided events,
    /// mutates nothing.
    ///
    /// # Errors
    ///
    /// Returns `A::Error` when the command violates a domain invariant.
    pub fn handle<C, const N: usize>(&self, cmd: C) -> Result<Events<EventOf<A>, N>, A::Error>
    where
        A: Handle<C, N>,
    {
        A::handle(self.state(), cmd)
    }

    /// Replay a single persisted event during rehydration.
    ///
    /// Moves the current state out via [`mem::replace`] and folds it through
    /// [`AggregateState::apply`] — no per-event clone, so the rehydration hot
    /// path copies nothing (only a cheap [`initial`](AggregateState::initial)
    /// placeholder is constructed). On version-validation failure the
    /// aggregate is left untouched (the placeholder is never installed).
    ///
    /// Takes a borrowed event reference so zero-copy codecs (rkyv, flatbuffers)
    /// can pass views directly from database buffers without cloning.
    ///
    /// The rehydration entry point: replays persisted events in strict version
    /// order (starting at [`Version::INITIAL`], strictly sequential) to
    /// reconstruct state. A repository drives this during load, but it is also
    /// the supported path for manual / no-store event sourcing — replay a
    /// known-good history `1..=n` to rebuild an aggregate with no persistence
    /// layer at all.
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
        // Move the state out and fold it through `apply` — no clone. If
        // `apply` panics (a state-machine bug), the state is left at
        // `initial()` (valid, never partially mutated).
        let taken = mem::replace(&mut self.state, A::State::initial());
        self.state = taken.apply(event);
        self.version = Some(version);
        Ok(())
    }

    /// Sync the aggregate after the store has durably persisted `events`.
    ///
    /// Call this **once** after the event store has committed `events`, with
    /// `version` set to the version of the **last** persisted event. It folds
    /// the two halves of post-persist bookkeeping — advancing the version and
    /// applying the events to state — into a single atomic call, so the version
    /// and state can never desync (the footgun of advancing one without the
    /// other is unrepresentable).
    ///
    /// The fold uses the no-clone [`mem::replace`] path, identical to
    /// [`replay`](Self::replay): if [`AggregateState::apply`] panics (a
    /// state-machine bug) the state is left at [`AggregateState::initial`] —
    /// valid, never partially mutated.
    ///
    /// This is the blessed post-persist seam: a repository drives it after a
    /// successful write, and a manual / no-store flow calls it after deciding
    /// events it considers committed.
    pub fn commit_persisted<const N: usize>(
        &mut self,
        version: Version,
        events: &Events<EventOf<A>, N>,
    ) {
        self.advance_version(version);
        self.apply_events(events);
    }

    /// Advance the version to reflect newly persisted events.
    ///
    /// Private primitive: advancing the version without also applying the
    /// matching events leaves state behind the version. The only caller is
    /// [`commit_persisted`](Self::commit_persisted), which always pairs it with
    /// [`apply_events`](Self::apply_events) atomically.
    const fn advance_version(&mut self, new_version: Version) {
        self.version = Some(new_version);
    }

    /// Apply already-persisted events to state without advancing the version.
    ///
    /// In-crate primitive (`pub(crate)`): [`commit_persisted`](Self::commit_persisted)
    /// pairs it with [`advance_version`](Self::advance_version), and the
    /// `testing` fixtures fold decided events without persisting (no version to
    /// advance). Not public — folding state without a version is exactly the
    /// desync the public API forbids.
    pub(crate) fn apply_events<const N: usize>(&mut self, events: &Events<EventOf<A>, N>) {
        for event in events {
            self.apply_event(event);
        }
    }

    /// Apply a single event to the aggregate state.
    ///
    /// Private primitive driving [`apply_events`](Self::apply_events). Moves the
    /// state out via [`mem::replace`] and folds it through `apply` — no clone.
    /// If `apply` panics (a state-machine bug), the state is left at
    /// [`AggregateState::initial`] (valid, never partially mutated).
    fn apply_event(&mut self, event: &EventOf<A>) {
        let taken = mem::replace(&mut self.state, A::State::initial());
        self.state = taken.apply(event);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test code: panic-safety test deliberately panics in apply()"
)]
mod purist_dispatch_tests {
    use super::{Aggregate, AggregateRoot, AggregateState, Handle};
    use crate::event::DomainEvent;
    use crate::events;
    use crate::events::Events;
    use crate::id::Id;
    use crate::message::Message;
    use crate::version::Version;

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct CtrId([u8; 8]);

    impl CtrId {
        fn new(n: u64) -> Self {
            Self(n.to_le_bytes())
        }
    }

    impl std::fmt::Display for CtrId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", u64::from_le_bytes(self.0))
        }
    }

    impl AsRef<[u8]> for CtrId {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    impl Id for CtrId {
        const BYTE_LEN: usize = 8;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum CtrEvent {
        Added(u64),
    }

    impl Message for CtrEvent {}

    impl DomainEvent for CtrEvent {
        fn name(&self) -> &'static str {
            match self {
                Self::Added(_) => "Added",
            }
        }
    }

    // Intentionally NOT `Clone` — replay/apply must not require it.
    #[derive(Debug)]
    struct CtrState {
        total: u64,
    }

    impl AggregateState for CtrState {
        type Event = CtrEvent;

        fn initial() -> Self {
            Self { total: 0 }
        }

        fn apply(mut self, event: &CtrEvent) -> Self {
            match event {
                CtrEvent::Added(n) => self.total = self.total.wrapping_add(*n),
            }
            self
        }
    }

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("counter error")]
    struct CtrError;

    struct Counter;

    impl Aggregate for Counter {
        type State = CtrState;
        type Error = CtrError;
        type Id = CtrId;
    }

    struct Add(u64);

    impl Handle<Add> for Counter {
        fn handle(state: &CtrState, cmd: Add) -> Result<Events<CtrEvent>, CtrError> {
            if cmd.0 == 0 {
                return Err(CtrError);
            }
            let _ = state.total;
            Ok(events![CtrEvent::Added(cmd.0)])
        }
    }

    #[test]
    fn dispatches_to_handle_on_the_marker() {
        let root = AggregateRoot::<Counter>::new(CtrId::new(1));
        let decided = root.handle(Add(5)).expect("ok");
        assert_eq!(
            decided.into_iter().collect::<Vec<_>>(),
            vec![CtrEvent::Added(5)]
        );
    }

    #[test]
    fn surfaces_domain_error_from_handle() {
        assert_eq!(
            AggregateRoot::<Counter>::new(CtrId::new(1)).handle(Add(0)),
            Err(CtrError)
        );
    }

    #[test]
    fn commit_persisted_advances_version_and_folds_state_atomically() {
        // The "can't desync" guarantee: one call must advance the version AND
        // fold every event into state. `CtrState: !Clone`, so this also proves
        // the no-clone `mem::replace` fold is used.
        let v2 = Version::new(2).expect("nonzero");
        let persisted: Events<CtrEvent, 1> = events![CtrEvent::Added(10), CtrEvent::Added(5)];
        let mut committed = AggregateRoot::<Counter>::new(CtrId::new(42));
        committed.commit_persisted(v2, &persisted);
        assert_eq!(committed.version(), Some(v2));
        assert_eq!(committed.state().total, 15);

        // The (version, state) reached via `commit_persisted` must equal the
        // (version, state) reached by replaying the same events one-by-one.
        let mut replayed = AggregateRoot::<Counter>::new(CtrId::new(42));
        replayed
            .replay(Version::INITIAL, &CtrEvent::Added(10))
            .expect("replay v1");
        replayed.replay(v2, &CtrEvent::Added(5)).expect("replay v2");
        assert_eq!(committed.version(), replayed.version());
        assert_eq!(committed.state().total, replayed.state().total);
    }

    // The following three tests cover the now-private primitives in isolation.
    // They were relocated in-crate from `tests/kernel_tests/aggregate_root_tests.rs`
    // (a separate crate, which can no longer reach `pub(crate)`/private methods)
    // when the post-persist pair was folded into the public `commit_persisted`.

    #[test]
    fn advance_version_sets_version_without_applying_state() {
        let mut agg = AggregateRoot::<Counter>::new(CtrId::new(1));
        assert_eq!(agg.version(), None);
        agg.advance_version(Version::INITIAL);
        assert_eq!(agg.version(), Version::new(1));
        // advance_version moves the version only; state is untouched.
        assert_eq!(agg.state().total, 0);
        // Idempotent on version: calling again with the same version is a no-op.
        agg.advance_version(Version::INITIAL);
        assert_eq!(agg.version(), Version::new(1));
    }

    #[test]
    fn apply_events_folds_state_without_advancing_version() {
        let mut agg = AggregateRoot::<Counter>::new(CtrId::new(1));
        let decided: Events<CtrEvent, 1> = events![CtrEvent::Added(2), CtrEvent::Added(3)];
        agg.apply_events(&decided);
        assert_eq!(agg.state().total, 5);
        // apply_events does NOT advance version — that is advance_version's job.
        assert_eq!(agg.version(), None);
    }

    #[test]
    fn apply_event_accumulates_state_without_advancing_version() {
        let mut agg = AggregateRoot::<Counter>::new(CtrId::new(1));
        agg.apply_event(&CtrEvent::Added(1));
        assert_eq!(agg.state().total, 1);
        agg.apply_event(&CtrEvent::Added(9));
        assert_eq!(agg.state().total, 10);
        // apply_event does NOT advance version.
        assert_eq!(agg.version(), None);
    }

    #[test]
    fn apply_events_mid_batch_panic_leaves_initial_state() {
        // Relocated in-crate from `tests/kernel_tests/security_tests.rs` (h5):
        // a panic mid-fold must leave state at `initial()` (no partial mutation)
        // and must not touch the version (apply_events does not set version).
        use std::panic;

        #[derive(Debug, Clone)]
        enum BoomEvent {
            Inc,
            Boom,
        }
        impl Message for BoomEvent {}
        impl DomainEvent for BoomEvent {
            fn name(&self) -> &'static str {
                match self {
                    Self::Inc => "Inc",
                    Self::Boom => "Boom",
                }
            }
        }

        #[derive(Default, Debug)]
        struct BoomState {
            count: u64,
        }
        impl AggregateState for BoomState {
            type Event = BoomEvent;
            fn initial() -> Self {
                Self::default()
            }
            fn apply(mut self, event: &BoomEvent) -> Self {
                match event {
                    BoomEvent::Inc => self.count += 1,
                    BoomEvent::Boom => panic!("boom in apply_events"),
                }
                self
            }
        }

        struct BoomAgg;
        impl Aggregate for BoomAgg {
            type State = BoomState;
            type Error = CtrError;
            type Id = CtrId;
        }

        let mut agg = AggregateRoot::<BoomAgg>::new(CtrId::new(1));
        let events: Events<BoomEvent, 1> = events![BoomEvent::Inc, BoomEvent::Boom];
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            agg.apply_events(&events);
        }));
        assert!(result.is_err(), "apply_events should have panicked");

        // mem::replace leaves a clean initial() placeholder when apply unwinds.
        assert_eq!(
            agg.state().count,
            0,
            "state must be left at initial() after a mid-batch panic"
        );
        assert_eq!(
            agg.version(),
            None,
            "version must remain None — apply_events does not set version"
        );
    }

    #[test]
    fn replay_folds_state_without_clone() {
        // `CtrState: !Clone` — this only compiles because `replay`/`apply`
        // move the state out (mem::replace) instead of cloning it. If a
        // `Clone` bound creeps back onto `AggregateState`, this fails to build.
        let mut root = AggregateRoot::<Counter>::new(CtrId::new(7));
        root.replay(Version::INITIAL, &CtrEvent::Added(10))
            .expect("replay v1");
        root.replay(Version::new(2).expect("nonzero"), &CtrEvent::Added(5))
            .expect("replay v2");
        assert_eq!(root.state().total, 15);
        assert_eq!(root.version(), Version::new(2));
    }
}
