//! Tests for `AggregateRoot::replay` — streaming rehydration.

use nexus::*;
use std::fmt;

// --- Minimal test domain ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RId(u64);
impl fmt::Display for RId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r-{}", self.0)
    }
}
impl Id for RId {}

#[derive(Debug, Clone, PartialEq)]
enum REvent {
    Added(String),
}
impl Message for REvent {}
impl DomainEvent for REvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Added(_) => "Added",
        }
    }
}

#[derive(Default, Debug)]
struct RState {
    items: Vec<String>,
}
impl AggregateState for RState {
    type Event = REvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &REvent) -> Self {
        match event {
            REvent::Added(s) => self.items.push(s.clone()),
        }
        self
    }
    fn name(&self) -> &'static str {
        "R"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("r error")]
struct RErr;

#[derive(Debug)]
struct RAgg;
impl Aggregate for RAgg {
    type State = RState;
    type Error = RErr;
    type Id = RId;
}

// =============================================================================
// replay tests
// =============================================================================

#[test]
fn replay_single_event_advances_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();

    assert_eq!(agg.version(), Version::from_persisted(1));
    assert_eq!(agg.state().items, vec!["a"]);
}

#[test]
fn replay_sequential_events() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();
    agg.replay(Version::from_persisted(2), &REvent::Added("b".into()))
        .unwrap();

    assert_eq!(agg.version(), Version::from_persisted(2));
    assert_eq!(agg.state().items, vec!["a", "b"]);
}

#[test]
fn replay_rejects_wrong_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    let err = agg
        .replay(Version::from_persisted(5), &REvent::Added("a".into()))
        .unwrap_err();

    match err {
        KernelError::VersionMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, Version::from_persisted(1));
            assert_eq!(actual, Version::from_persisted(5));
        }
        other @ KernelError::RehydrationLimitExceeded { .. } => {
            panic!("expected VersionMismatch, got {other:?}")
        }
    }
}

#[test]
fn replay_rejects_duplicate_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();

    let err = agg
        .replay(Version::from_persisted(1), &REvent::Added("b".into()))
        .unwrap_err();

    match err {
        KernelError::VersionMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, Version::from_persisted(2));
            assert_eq!(actual, Version::from_persisted(1));
        }
        other @ KernelError::RehydrationLimitExceeded { .. } => {
            panic!("expected VersionMismatch, got {other:?}")
        }
    }
}

#[test]
fn replay_then_apply_continues_from_correct_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();

    // Now apply a new event (uncommitted)
    agg.apply(REvent::Added("b".into()));

    assert_eq!(agg.version(), Version::from_persisted(1)); // persisted version unchanged
    assert_eq!(agg.current_version(), Version::from_persisted(2)); // includes uncommitted

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].version(), Version::from_persisted(2));
}

#[test]
fn replay_does_not_record_uncommitted() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();
    agg.replay(Version::from_persisted(2), &REvent::Added("b".into()))
        .unwrap();

    let events = agg.take_uncommitted_events();
    assert!(
        events.is_empty(),
        "replay must not produce uncommitted events"
    );
}

#[test]
fn replay_enforces_rehydration_limit() {
    // Aggregate with tiny rehydration limit
    #[derive(Debug)]
    struct SmallLimitAgg;
    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct SmallErr;
    impl Aggregate for SmallLimitAgg {
        type State = RState;
        type Error = SmallErr;
        type Id = RId;
        const MAX_REHYDRATION_EVENTS: usize = 3;
    }

    let mut agg = AggregateRoot::<SmallLimitAgg>::new(RId(1));
    agg.replay(Version::from_persisted(1), &REvent::Added("a".into()))
        .unwrap();
    agg.replay(Version::from_persisted(2), &REvent::Added("b".into()))
        .unwrap();
    agg.replay(Version::from_persisted(3), &REvent::Added("c".into()))
        .unwrap();

    // 4th event exceeds the limit of 3
    let err = agg
        .replay(Version::from_persisted(4), &REvent::Added("d".into()))
        .unwrap_err();

    match err {
        KernelError::RehydrationLimitExceeded { max, .. } => {
            assert_eq!(max, 3);
        }
        other @ KernelError::VersionMismatch { .. } => {
            panic!("expected RehydrationLimitExceeded, got {other:?}")
        }
    }
}
