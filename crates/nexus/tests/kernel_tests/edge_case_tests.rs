//! Edge case tests for the kernel.
//!
//! These cover:
//! - New aggregate without replay has default state and `None` version
//! - `replay` rejects first event not at version 1
//! - Version after replay sequence
//! - `advance_version` + `apply_events` after replay (rehydrate then advance)
//! - `Events` ref iteration
//! - Multiple replays in sequence
//! - `replay` then `advance_version` + `apply_events` continues from correct version

use nexus::AggregateRoot;
use nexus::AggregateState;
use nexus::DomainEvent;
use nexus::Events;
use nexus::KernelError;
use nexus::Message;
use nexus::Version;
use nexus::{Aggregate, Id, events};
use std::fmt;

// --- Minimal test domain ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TId(String);

impl TId {
    fn new(v: u64) -> Self {
        Self(format!("t-{v}"))
    }
}

impl fmt::Display for TId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<[u8]> for TId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for TId {}

#[derive(Debug, Clone, PartialEq)]
struct Added(String);

#[derive(Debug, Clone, PartialEq)]
struct Removed;

#[derive(Debug, Clone, PartialEq)]
enum TEvent {
    Added(Added),
    Removed(Removed),
}

impl Message for TEvent {}

impl DomainEvent for TEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Added(_) => "Added",
            Self::Removed(_) => "Removed",
        }
    }
}

#[derive(Default, Debug, Clone)]
struct TState {
    items: Vec<String>,
}

impl AggregateState for TState {
    type Event = TEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &TEvent) -> Self {
        match event {
            TEvent::Added(e) => self.items.push(e.0.clone()),
            TEvent::Removed(_) => {
                self.items.pop();
            }
        }
        self
    }
}

#[derive(Debug)]
struct TAgg;

#[derive(Debug, thiserror::Error)]
#[error("test error")]
struct TErr;

impl Aggregate for TAgg {
    type State = TState;
    type Error = TErr;
    type Id = TId;
}

// =============================================================================
// 1. New aggregate without replay has default state and None version
// =============================================================================

#[test]
fn new_aggregate_without_replay_has_default_state_and_none_version() {
    let agg = AggregateRoot::<TAgg>::new(TId::new(1));
    assert_eq!(agg.version(), None);
    assert!(agg.state().items.is_empty());
}

// =============================================================================
// 2. replay rejects first event not at version 1
// =============================================================================

#[test]
fn replay_rejects_first_event_not_version_1() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let version_5 = Version::new(5).expect("5 is non-zero");
    let err = agg
        .replay(version_5, &TEvent::Added(Added("x".into())))
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, Version::INITIAL);
            assert_eq!(actual, version_5);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn replay_rejects_version_zero() {
    // Version::new(0) returns None, so we cannot even construct the call.
    assert!(Version::new(0).is_none());
}

// =============================================================================
// 3. Version after replay sequence
// =============================================================================

#[test]
fn version_tracks_replay_sequence() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let v1 = Version::new(1).expect("non-zero");
    let v2 = Version::new(2).expect("non-zero");
    let v3 = Version::new(3).expect("non-zero");

    agg.replay(v1, &TEvent::Added(Added("a".into()))).unwrap();
    assert_eq!(agg.version(), Some(v1));

    agg.replay(v2, &TEvent::Added(Added("b".into()))).unwrap();
    assert_eq!(agg.version(), Some(v2));

    agg.replay(v3, &TEvent::Added(Added("c".into()))).unwrap();
    assert_eq!(agg.version(), Some(v3));

    assert_eq!(agg.state().items, vec!["a", "b", "c"]);
}

// =============================================================================
// 4. advance_version + apply_events after replay (rehydrate then advance)
// =============================================================================

#[test]
fn advance_version_and_apply_events_after_replay() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let v1 = Version::new(1).expect("non-zero");
    let v2 = Version::new(2).expect("non-zero");
    let v3 = Version::new(3).expect("non-zero");
    let v5 = Version::new(5).expect("non-zero");

    // Rehydrate with 3 events
    agg.replay(v1, &TEvent::Added(Added("a".into()))).unwrap();
    agg.replay(v2, &TEvent::Added(Added("b".into()))).unwrap();
    agg.replay(v3, &TEvent::Added(Added("c".into()))).unwrap();
    assert_eq!(agg.version(), Some(v3));

    // Simulate persisting 2 new events, then advancing
    let new_events: Events<_, 1> =
        events![TEvent::Added(Added("d".into())), TEvent::Removed(Removed)];
    agg.advance_version(v5);
    agg.apply_events(&new_events);

    assert_eq!(agg.version(), Some(v5));
    // State: [a, b, c] -> Added(d) -> [a, b, c, d] -> Removed -> [a, b, c]
    assert_eq!(agg.state().items, vec!["a", "b", "c"]);
}

// Separate clean test for advance + apply correctness
#[test]
fn advance_version_and_apply_events_state_correctness() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let v1 = Version::new(1).expect("non-zero");
    let v2 = Version::new(2).expect("non-zero");

    agg.replay(v1, &TEvent::Added(Added("a".into()))).unwrap();
    agg.replay(v2, &TEvent::Added(Added("b".into()))).unwrap();
    assert_eq!(agg.version(), Some(v2));
    assert_eq!(agg.state().items, vec!["a", "b"]);

    // Simulate: command handler produced 2 events, repository persisted them
    let decided: Events<_, 1> = events![
        TEvent::Added(Added("c".into())),
        TEvent::Added(Added("d".into()))
    ];
    let v4 = Version::new(4).expect("non-zero");
    agg.advance_version(v4);
    agg.apply_events(&decided);

    assert_eq!(agg.version(), Some(v4));
    assert_eq!(agg.state().items, vec!["a", "b", "c", "d"]);
}

// =============================================================================
// 5. Events ref iteration (still valid)
// =============================================================================

#[test]
fn events_ref_iteration() {
    let mut events: Events<_, 1> = Events::new(TEvent::Added(Added("a".into())));
    events.add(TEvent::Removed(Removed));

    // Iterate by reference (non-consuming)
    let names: Vec<&str> = (&events).into_iter().map(DomainEvent::name).collect();
    assert_eq!(names, vec!["Added", "Removed"]);

    // Original still usable
    assert_eq!(events.len(), 2);
}

#[test]
fn events_iter_method() {
    let events: Events<_, 2> = events![
        TEvent::Added(Added("x".into())),
        TEvent::Added(Added("y".into())),
        TEvent::Removed(Removed)
    ];

    let names: Vec<&str> = events.iter().map(DomainEvent::name).collect();
    assert_eq!(names, vec!["Added", "Added", "Removed"]);
    assert_eq!(events.len(), 3);
}

// =============================================================================
// 6. Multiple replays in sequence
// =============================================================================

#[test]
fn multiple_replays_in_strict_sequence() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(42));

    for i in 1..=10 {
        let version = Version::new(i).expect("non-zero");
        agg.replay(version, &TEvent::Added(Added(format!("item-{i}"))))
            .unwrap();
    }

    assert_eq!(agg.version(), Version::new(10));
    assert_eq!(agg.state().items.len(), 10);
    assert_eq!(agg.state().items[0], "item-1");
    assert_eq!(agg.state().items[9], "item-10");
}

#[test]
fn replay_rejects_gap_in_sequence() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let v1 = Version::new(1).expect("non-zero");
    let v3 = Version::new(3).expect("non-zero");

    agg.replay(v1, &TEvent::Added(Added("a".into()))).unwrap();

    // Skip version 2 -> should fail
    let err = agg
        .replay(v3, &TEvent::Added(Added("c".into())))
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            let v2 = Version::new(2).expect("non-zero");
            assert_eq!(expected, v2);
            assert_eq!(actual, v3);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn replay_rejects_duplicate_version() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let v1 = Version::new(1).expect("non-zero");

    agg.replay(v1, &TEvent::Added(Added("a".into()))).unwrap();

    // Replay same version again -> should fail
    let err = agg
        .replay(v1, &TEvent::Added(Added("dup".into())))
        .unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            let v2 = Version::new(2).expect("non-zero");
            assert_eq!(expected, v2);
            assert_eq!(actual, v1);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// 7. replay then advance_version + apply_events continues from correct version
// =============================================================================

#[test]
fn replay_then_advance_and_apply_continues_correctly() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(7));
    let v1 = Version::new(1).expect("non-zero");
    let v2 = Version::new(2).expect("non-zero");
    let v3 = Version::new(3).expect("non-zero");

    // Rehydrate 2 events
    agg.replay(v1, &TEvent::Added(Added("x".into()))).unwrap();
    agg.replay(v2, &TEvent::Added(Added("y".into()))).unwrap();
    assert_eq!(agg.version(), Some(v2));

    // Simulate persisting 1 new event at version 3
    let decided: Events<_, 0> = events![TEvent::Added(Added("z".into()))];
    agg.advance_version(v3);
    agg.apply_events(&decided);

    assert_eq!(agg.version(), Some(v3));
    assert_eq!(agg.state().items, vec!["x", "y", "z"]);

    // After advance, replay should still validate from v3's successor (v4)
    let v4 = Version::new(4).expect("non-zero");
    agg.replay(v4, &TEvent::Added(Added("w".into()))).unwrap();
    assert_eq!(agg.version(), Some(v4));
    assert_eq!(agg.state().items, vec!["x", "y", "z", "w"]);
}

#[test]
fn advance_version_on_fresh_aggregate() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    assert_eq!(agg.version(), None);

    // Simulate: first-ever command produces events, repository persists them
    let decided: Events<_, 0> = events![TEvent::Added(Added("first".into()))];
    let v1 = Version::new(1).expect("non-zero");
    agg.advance_version(v1);
    agg.apply_events(&decided);

    assert_eq!(agg.version(), Some(v1));
    assert_eq!(agg.state().items, vec!["first"]);
}

#[test]
fn apply_events_with_single_event() {
    let mut agg = AggregateRoot::<TAgg>::new(TId::new(1));
    let decided: Events<_, 0> = events![TEvent::Added(Added("only".into()))];
    let v1 = Version::new(1).expect("non-zero");

    agg.advance_version(v1);
    agg.apply_events(&decided);

    assert_eq!(agg.state().items, vec!["only"]);
}

#[test]
fn id_is_preserved() {
    let agg = AggregateRoot::<TAgg>::new(TId::new(42));
    assert_eq!(*agg.id(), TId::new(42));
}
