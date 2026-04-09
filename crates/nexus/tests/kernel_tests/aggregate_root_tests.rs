use std::fmt;
use std::num::NonZeroUsize;

use nexus::Aggregate;
use nexus::AggregateEntity;
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

impl Id for TestId {}

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

struct CounterAggregate;

impl Aggregate for CounterAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = TestId;
}

// ---------------------------------------------------------------------------
// AggregateEntity newtype (delegation pattern)
// ---------------------------------------------------------------------------

struct Counter(AggregateRoot<Self>);

impl fmt::Debug for Counter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Counter").field(&self.0).finish()
    }
}

impl Aggregate for Counter {
    type State = CounterState;
    type Error = CounterError;
    type Id = TestId;
}

impl AggregateEntity for Counter {
    fn root(&self) -> &AggregateRoot<Self> {
        &self.0
    }

    fn root_mut(&mut self) -> &mut AggregateRoot<Self> {
        &mut self.0
    }
}

impl Counter {
    fn new(id: TestId) -> Self {
        Self(AggregateRoot::new(id))
    }
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
    fn handle(&self, _cmd: Increment) -> Result<Events<CounterEvent>, CounterError> {
        Ok(events![CounterEvent::Incremented])
    }
}

impl Handle<IncrementBy> for Counter {
    fn handle(&self, cmd: IncrementBy) -> Result<Events<CounterEvent>, CounterError> {
        if cmd.amount == 0 {
            return Err(CounterError::ZeroIncrement);
        }
        Ok(events![CounterEvent::IncrementedBy(cmd.amount)])
    }
}

impl Handle<Decrement> for Counter {
    fn handle(&self, _cmd: Decrement) -> Result<Events<CounterEvent>, CounterError> {
        if self.state().value <= 0 {
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
    let agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    assert_eq!(agg.version(), None);
}

#[test]
fn new_aggregate_has_initial_state() {
    let agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    assert_eq!(agg.state().value, 0);
}

// ---------------------------------------------------------------------------
// Tests: aggregate id accessible
// ---------------------------------------------------------------------------

#[test]
fn aggregate_id_is_accessible() {
    let agg = AggregateRoot::<CounterAggregate>::new(TestId("abc".into()));
    assert_eq!(agg.id(), &TestId("abc".into()));
}

// ---------------------------------------------------------------------------
// Tests: replay advances version and mutates state
// ---------------------------------------------------------------------------

#[test]
fn replay_single_event_advances_version_and_state() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    agg.replay(Version::INITIAL, &CounterEvent::Incremented)
        .unwrap();
    assert_eq!(agg.version(), Some(Version::INITIAL));
    assert_eq!(agg.state().value, 1);
}

#[test]
fn replay_multiple_events_sequentially() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
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
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
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
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
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
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
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
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
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
    let counter = Counter::new(TestId("1".into()));
    let decided = counter.handle(Increment).unwrap();
    assert_eq!(decided.len(), 1);
    assert!(matches!(
        decided.iter().next(),
        Some(CounterEvent::Incremented)
    ));
}

#[test]
fn handle_increment_by_returns_event_with_amount() {
    let counter = Counter::new(TestId("1".into()));
    let decided = counter.handle(IncrementBy { amount: 42 }).unwrap();
    assert_eq!(decided.len(), 1);
    assert!(matches!(
        decided.iter().next(),
        Some(CounterEvent::IncrementedBy(42))
    ));
}

#[test]
fn handle_rejects_invalid_command() {
    let counter = Counter::new(TestId("1".into()));
    let err = counter.handle(Decrement).unwrap_err();
    assert!(matches!(err, CounterError::WouldGoNegative));
}

#[test]
fn handle_rejects_zero_increment() {
    let counter = Counter::new(TestId("1".into()));
    let err = counter.handle(IncrementBy { amount: 0 }).unwrap_err();
    assert!(matches!(err, CounterError::ZeroIncrement));
}

#[test]
fn handle_uses_current_state_for_decision() {
    let mut counter = Counter::new(TestId("1".into()));
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
// Tests: advance_version + apply_events (post-persistence workflow)
// ---------------------------------------------------------------------------

#[test]
fn advance_version_updates_version() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    assert_eq!(agg.version(), None);

    agg.advance_version(Version::new(1).unwrap());
    assert_eq!(agg.version(), Version::new(1));
}

#[test]
fn apply_events_updates_state_without_changing_version() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::Incremented];
    agg.apply_events(&decided);

    assert_eq!(agg.state().value, 2);
    // apply_events does NOT advance version — that is advance_version's job.
    assert_eq!(agg.version(), None);
}

#[test]
fn advance_version_then_apply_events_full_workflow() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));

    // Simulate: handle -> persist -> advance + apply.
    let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::IncrementedBy(9)];
    let new_version = Version::new(2).unwrap();
    agg.advance_version(new_version);
    agg.apply_events(&decided);

    assert_eq!(agg.version(), Version::new(2));
    assert_eq!(agg.state().value, 10);
}

#[test]
fn advance_version_then_replay_continues_from_advanced_version() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));

    // First, replay version 1.
    agg.replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();

    // Simulate persistence of two more events (versions 2, 3).
    let decided: Events<_, 1> = events![CounterEvent::Incremented, CounterEvent::Incremented];
    agg.advance_version(Version::new(3).unwrap());
    agg.apply_events(&decided);

    assert_eq!(agg.version(), Version::new(3));
    assert_eq!(agg.state().value, 3);

    // Further replay must continue from version 4.
    agg.replay(Version::new(4).unwrap(), &CounterEvent::Decremented)
        .unwrap();
    assert_eq!(agg.version(), Version::new(4));
    assert_eq!(agg.state().value, 2);
}

// ---------------------------------------------------------------------------
// Tests: apply_event (single-event state advancement)
// ---------------------------------------------------------------------------

#[test]
fn apply_event_updates_state_single_increment() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    assert_eq!(agg.state().value, 0);

    agg.apply_event(&CounterEvent::Incremented);
    assert_eq!(agg.state().value, 1);
}

#[test]
fn apply_event_called_twice_accumulates_state() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    assert_eq!(agg.state().value, 0);

    agg.apply_event(&CounterEvent::Incremented);
    assert_eq!(agg.state().value, 1);

    agg.apply_event(&CounterEvent::IncrementedBy(9));
    assert_eq!(agg.state().value, 10);
}

#[test]
fn apply_event_does_not_change_version() {
    let mut agg = AggregateRoot::<CounterAggregate>::new(TestId("1".into()));
    agg.apply_event(&CounterEvent::Incremented);

    // apply_event does NOT advance version — that is advance_version's job.
    assert_eq!(agg.version(), None);
}

// ---------------------------------------------------------------------------
// Tests: AggregateEntity delegation
// ---------------------------------------------------------------------------

#[test]
fn entity_id_delegates_to_root() {
    let counter = Counter::new(TestId("e1".into()));
    assert_eq!(counter.id(), &TestId("e1".into()));
}

#[test]
fn entity_state_delegates_to_root() {
    let counter = Counter::new(TestId("e1".into()));
    assert_eq!(counter.state().value, 0);
}

#[test]
fn entity_version_delegates_to_root() {
    let counter = Counter::new(TestId("e1".into()));
    assert_eq!(counter.version(), None);
}

#[test]
fn entity_replay_delegates_to_root() {
    let mut counter = Counter::new(TestId("e1".into()));
    counter
        .replay(Version::new(1).unwrap(), &CounterEvent::Incremented)
        .unwrap();
    assert_eq!(counter.version(), Version::new(1));
    assert_eq!(counter.state().value, 1);
}

#[test]
fn entity_replay_rejects_gap_same_as_root() {
    let mut counter = Counter::new(TestId("e1".into()));
    let err = counter
        .replay(Version::new(2).unwrap(), &CounterEvent::Incremented)
        .unwrap_err();
    assert!(matches!(
        err,
        KernelError::VersionMismatch {
            expected,
            actual,
        } if expected == Version::INITIAL && actual == Version::new(2).unwrap()
    ));
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
