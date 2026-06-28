use std::fmt;
use std::num::NonZeroUsize;

use nexus::Aggregate;
use nexus::AggregateRoot;
use nexus::AggregateState;
use nexus::DomainEvent;
use nexus::Events;
use nexus::Handle;
use nexus::Id;
use nexus::KernelError;
use nexus::Message;
use nexus::Version;
use nexus::events;

// ---------------------------------------------------------------------------
// Self-contained test domain
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

#[derive(Debug, Clone)]
enum CounterEvent {
    Incremented,
    Decremented,
    IncrementedBy(u64),
}

impl Message for CounterEvent {}

impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Incremented => "Incremented",
            Self::Decremented => "Decremented",
            Self::IncrementedBy(_) => "IncrementedBy",
        }
    }
}

#[derive(Default, Debug, Clone)]
struct CounterState {
    value: i64,
}

impl AggregateState for CounterState {
    type Event = CounterEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &CounterEvent) -> Self {
        match event {
            CounterEvent::Incremented => self.value += 1,
            CounterEvent::Decremented => self.value -= 1,
            CounterEvent::IncrementedBy(n) => self.value += i64::try_from(*n).unwrap_or(i64::MAX),
        }
        self
    }
}

#[derive(Debug, thiserror::Error)]
enum CounterError {
    #[error("counter would go negative")]
    WouldGoNegative,
    #[error("increment amount must be positive")]
    ZeroIncrement,
}

// `Counter` is a bare marker aggregate. Command handlers are pure associated
// functions implemented directly on the marker; the loaded `AggregateRoot<Counter>`
// dispatches to them via its inherent `handle`.
struct Counter;

impl Aggregate for Counter {
    type State = CounterState;
    type Error = CounterError;
    type Id = TestId;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

struct Increment;

struct IncrementBy {
    amount: u64,
}

struct Decrement;

impl Handle<Increment> for Counter {
    fn handle(
        _state: &CounterState,
        _cmd: Increment,
    ) -> Result<Events<CounterEvent>, CounterError> {
        Ok(events![CounterEvent::Incremented])
    }
}

impl Handle<IncrementBy> for Counter {
    fn handle(
        _state: &CounterState,
        cmd: IncrementBy,
    ) -> Result<Events<CounterEvent>, CounterError> {
        if cmd.amount == 0 {
            return Err(CounterError::ZeroIncrement);
        }
        Ok(events![CounterEvent::IncrementedBy(cmd.amount)])
    }
}

impl Handle<Decrement> for Counter {
    fn handle(state: &CounterState, _cmd: Decrement) -> Result<Events<CounterEvent>, CounterError> {
        if state.value <= 0 {
            return Err(CounterError::WouldGoNegative);
        }
        Ok(events![CounterEvent::Decremented])
    }
}

// ---------------------------------------------------------------------------
// Tests: new aggregate initial state
// ---------------------------------------------------------------------------

#[test]
fn new_aggregate_has_none_version() {
    let agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    assert_eq!(agg.version(), None);
}

#[test]
fn new_aggregate_has_initial_state() {
    let agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    assert_eq!(agg.state().value, 0);
}

// ---------------------------------------------------------------------------
// Tests: aggregate id accessible
// ---------------------------------------------------------------------------

#[test]
fn aggregate_id_is_accessible() {
    let agg = AggregateRoot::<Counter>::new(TestId("abc".into()));
    assert_eq!(agg.id(), &TestId("abc".into()));
}

// ---------------------------------------------------------------------------
// Tests: replay advances version and mutates state
// ---------------------------------------------------------------------------

#[test]
fn replay_single_event_advances_version_and_state() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    agg.replay(Version::INITIAL, &CounterEvent::Incremented)
        .unwrap();
    assert_eq!(agg.version(), Some(Version::INITIAL));
    assert_eq!(agg.state().value, 1);
}

#[test]
fn replay_multiple_events_sequentially() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();
    agg.replay(Version::new(2).unwrap(), &CounterEvent::Incremented)
        .unwrap();
    agg.replay(Version::new(3).unwrap(), &CounterEvent::Decremented)
        .unwrap();

    assert_eq!(agg.version(), Version::new(3));
    assert_eq!(agg.state().value, 1);
}

// ---------------------------------------------------------------------------
// Tests: replay rejects version gaps
// ---------------------------------------------------------------------------

#[test]
fn replay_rejects_version_gap() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    let err = agg
        .replay(Version::new(3).unwrap(), &CounterEvent::Incremented)
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, Version::new(2).unwrap());
            assert_eq!(actual, Version::new(3).unwrap());
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn replay_rejects_gap_on_fresh_aggregate() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    let err = agg
        .replay(Version::new(5).unwrap(), &CounterEvent::Incremented)
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, Version::INITIAL);
            assert_eq!(actual, Version::new(5).unwrap());
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tests: replay rejects duplicate versions
// ---------------------------------------------------------------------------

#[test]
fn replay_rejects_duplicate_version() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    let err = agg
        .replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, Version::new(2).unwrap());
            assert_eq!(actual, Version::new(1).unwrap());
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tests: replay preserves state on error
// ---------------------------------------------------------------------------

#[test]
fn replay_does_not_mutate_state_on_version_gap() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    let _ = agg.replay(Version::new(3).unwrap(), &CounterEvent::Incremented);

    assert_eq!(agg.version(), Version::new(1));
    assert_eq!(agg.state().value, 1);
}

// ---------------------------------------------------------------------------
// Tests: Handle trait (decide pattern)
// ---------------------------------------------------------------------------

#[test]
fn handle_increment_returns_event() {
    let counter = AggregateRoot::<Counter>::new(TestId("1".into()));
    let decided = counter.handle(Increment).unwrap();
    assert_eq!(decided.len(), 1);
    assert!(matches!(
        decided.iter().next(),
        Some(CounterEvent::Incremented)
    ));
}

#[test]
fn handle_increment_by_returns_event_with_amount() {
    let counter = AggregateRoot::<Counter>::new(TestId("1".into()));
    let decided = counter.handle(IncrementBy { amount: 42 }).unwrap();
    assert_eq!(decided.len(), 1);
    assert!(matches!(
        decided.iter().next(),
        Some(CounterEvent::IncrementedBy(42))
    ));
}

#[test]
fn handle_rejects_invalid_command() {
    let counter = AggregateRoot::<Counter>::new(TestId("1".into()));
    let err = counter.handle(Decrement).unwrap_err();
    assert!(matches!(err, CounterError::WouldGoNegative));
}

#[test]
fn handle_rejects_zero_increment() {
    let counter = AggregateRoot::<Counter>::new(TestId("1".into()));
    let err = counter.handle(IncrementBy { amount: 0 }).unwrap_err();
    assert!(matches!(err, CounterError::ZeroIncrement));
}

#[test]
fn handle_uses_current_state_for_decision() {
    let mut counter = AggregateRoot::<Counter>::new(TestId("1".into()));
    // Replay an increment so decrement becomes valid.
    counter
        .replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    let decided = counter.handle(Decrement).unwrap();
    assert_eq!(decided.len(), 1);
    assert!(matches!(
        decided.iter().next(),
        Some(CounterEvent::Decremented)
    ));
}

// ---------------------------------------------------------------------------
// Tests: commit_persisted (post-persistence workflow)
//
// The lower-level `advance_version` / `apply_events` / `apply_event` primitives
// are now private to the `nexus` crate; their in-isolation behavior is covered
// by in-crate tests in `crates/nexus/src/aggregate.rs`. These cross-crate tests
// exercise the public `commit_persisted` seam the repository drives.
// ---------------------------------------------------------------------------

#[test]
fn commit_persisted_advances_version_and_folds_state() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));

    // Simulate: handle -> persist -> commit_persisted.
    let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::IncrementedBy(9)];
    let new_version = Version::new(2).unwrap();
    agg.commit_persisted(new_version, &decided);

    assert_eq!(agg.version(), Version::new(2));
    assert_eq!(agg.state().value, 10);
}

#[test]
fn commit_persisted_then_replay_continues_from_committed_version() {
    let mut agg = AggregateRoot::<Counter>::new(TestId("1".into()));

    // First, replay version 1.
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    // Simulate persistence of two more events (versions 2, 3).
    let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::Incremented];
    agg.commit_persisted(Version::new(3).unwrap(), &decided);

    assert_eq!(agg.version(), Version::new(3));
    assert_eq!(agg.state().value, 3);

    // Further replay must continue from version 4.
    agg.replay(Version::new(4).unwrap(), &CounterEvent::Decremented)
        .unwrap();
    assert_eq!(agg.version(), Version::new(4));
    assert_eq!(agg.state().value, 2);
}

// ---------------------------------------------------------------------------
// Tests: rehydration limit
// ---------------------------------------------------------------------------

struct TinyLimitAggregate;

#[allow(clippy::unwrap_used, reason = "3 is non-zero by inspection")]
impl Aggregate for TinyLimitAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = TestId;
    const MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(3).unwrap();
}

#[test]
fn replay_respects_rehydration_limit() {
    let mut agg = AggregateRoot::<TinyLimitAggregate>::new(TestId("1".into()));
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();
    agg.replay(Version::new(2).unwrap(), &CounterEvent::Incremented)
        .unwrap();
    agg.replay(Version::new(3).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    // Version 4 exceeds the limit of 3.
    let err = agg
        .replay(Version::new(4).unwrap(), &CounterEvent::Incremented)
        .unwrap_err();
    assert!(matches!(
        err,
        KernelError::RehydrationLimitExceeded { max: 3 }
    ));
}

// ---------------------------------------------------------------------------
// Tests: Version::new returns None for zero
// ---------------------------------------------------------------------------

#[test]
fn version_new_rejects_zero() {
    assert!(Version::new(0).is_none());
}

#[test]
fn version_new_accepts_nonzero() {
    let v = Version::new(42).unwrap();
    assert_eq!(v.as_u64(), 42);
}

#[test]
fn version_initial_is_one() {
    assert_eq!(Version::INITIAL.as_u64(), 1);
}

// ---------------------------------------------------------------------------
// Tests: restore from snapshot
// ---------------------------------------------------------------------------

#[test]
fn restore_creates_root_at_given_state_and_version() {
    let id = TestId("1".into());
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let root = AggregateRoot::<Counter>::restore(id.clone(), state, version);

    assert_eq!(root.id(), &id);
    assert_eq!(root.state().value, 42);
    assert_eq!(root.version(), Some(version));
}

#[test]
fn restore_then_replay_continues_from_snapshot_version() {
    let id = TestId("1".into());
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let mut root = AggregateRoot::<Counter>::restore(id, state, version);

    // Next replay must be version 11
    let result = root.replay(Version::new(11).unwrap(), &CounterEvent::Incremented);
    assert!(result.is_ok());
    assert_eq!(root.state().value, 43);
    assert_eq!(root.version(), Some(Version::new(11).unwrap()));
}

#[test]
fn restore_then_replay_rejects_wrong_version() {
    let id = TestId("1".into());
    let state = CounterState { value: 42 };
    let version = Version::new(10).unwrap();

    let mut root = AggregateRoot::<Counter>::restore(id, state, version);

    // Replaying version 10 (not 11) must fail
    let result = root.replay(Version::new(10).unwrap(), &CounterEvent::Incremented);
    assert!(result.is_err());
}
