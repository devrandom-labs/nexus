//! Edge case tests for the kernel.
//!
//! These cover the gaps identified in our testing audit:
//! - apply_events() (multi-event variant)
//! - load_from_events with empty iterator
//! - load_from_events starting from wrong version
//! - current_version after rehydrate + new events
//! - take, apply, take again pattern
//! - Events ref iteration

use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;
use nexus::kernel::events::Events;
use nexus::kernel::version::VersionedEvent;
use std::fmt;

// --- Minimal test domain ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TId(u64);
impl fmt::Display for TId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t-{}", self.0)
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
            TEvent::Added(_) => "Added",
            TEvent::Removed(_) => "Removed",
        }
    }
}

#[derive(Default, Debug)]
struct TState {
    items: Vec<String>,
}
impl AggregateState for TState {
    type Event = TEvent;
    fn apply(&mut self, event: &TEvent) {
        match event {
            TEvent::Added(e) => self.items.push(e.0.clone()),
            TEvent::Removed(_) => {
                self.items.pop();
            }
        }
    }
    fn name(&self) -> &'static str {
        "T"
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
// apply_events (multi-event variant) — was untested
// =============================================================================

#[test]
fn apply_events_multiple() {
    let mut agg = AggregateRoot::<TAgg>::new(TId(1));
    agg.apply_events([
        TEvent::Added(Added("a".into())),
        TEvent::Added(Added("b".into())),
        TEvent::Removed(Removed),
    ]);
    assert_eq!(agg.state().items, vec!["a".to_string()]);
    assert_eq!(agg.current_version(), Version::from_persisted(3));
}

#[test]
fn apply_events_empty_iterator_is_noop() {
    let mut agg = AggregateRoot::<TAgg>::new(TId(1));
    agg.apply_events(std::iter::empty());
    assert_eq!(agg.version(), Version::INITIAL);
    assert_eq!(agg.current_version(), Version::INITIAL);
    assert!(agg.take_uncommitted_events().is_empty());
}

// =============================================================================
// load_from_events edge cases
// =============================================================================

#[test]
fn load_from_events_empty_iterator_returns_default() {
    let agg = AggregateRoot::<TAgg>::load_from_events(TId(1), Vec::new()).unwrap();
    assert_eq!(agg.version(), Version::INITIAL);
    assert!(agg.state().items.is_empty());
}

#[test]
fn load_from_events_rejects_first_event_not_version_1() {
    let events = vec![VersionedEvent::from_persisted(
        Version::from_persisted(5), // should be 1!
        TEvent::Added(Added("x".into())),
    )];
    let err = AggregateRoot::<TAgg>::load_from_events(TId(1), events).unwrap_err();
    match err {
        KernelError::VersionMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, Version::from_persisted(1));
            assert_eq!(actual, Version::from_persisted(5));
        }
    }
}

// =============================================================================
// current_version after rehydrate + new events
// =============================================================================

#[test]
fn current_version_after_rehydrate_plus_new_events() {
    let history = vec![
        VersionedEvent::from_persisted(
            Version::from_persisted(1),
            TEvent::Added(Added("a".into())),
        ),
        VersionedEvent::from_persisted(
            Version::from_persisted(2),
            TEvent::Added(Added("b".into())),
        ),
        VersionedEvent::from_persisted(
            Version::from_persisted(3),
            TEvent::Added(Added("c".into())),
        ),
    ];
    let mut agg = AggregateRoot::<TAgg>::load_from_events(TId(1), history).unwrap();

    assert_eq!(agg.version(), Version::from_persisted(3)); // persisted
    assert_eq!(agg.current_version(), Version::from_persisted(3)); // no uncommitted yet

    agg.apply_event(TEvent::Added(Added("d".into())));
    agg.apply_event(TEvent::Removed(Removed));

    assert_eq!(agg.version(), Version::from_persisted(3)); // persisted unchanged
    assert_eq!(agg.current_version(), Version::from_persisted(5)); // 3 + 2 uncommitted

    let events = agg.take_uncommitted_events();
    assert_eq!(events[0].version(), Version::from_persisted(4));
    assert_eq!(events[1].version(), Version::from_persisted(5));
}

// =============================================================================
// take, apply, take pattern
// =============================================================================

#[test]
fn take_then_apply_then_take_again() {
    let mut agg = AggregateRoot::<TAgg>::new(TId(1));

    // First batch
    agg.apply_event(TEvent::Added(Added("first".into())));
    let batch1 = agg.take_uncommitted_events();
    assert_eq!(batch1.len(), 1);
    assert_eq!(batch1[0].version(), Version::from_persisted(1));

    // Second batch — versions should continue from where we left off
    agg.apply_event(TEvent::Added(Added("second".into())));
    agg.apply_event(TEvent::Added(Added("third".into())));
    let batch2 = agg.take_uncommitted_events();
    assert_eq!(batch2.len(), 2);
    assert_eq!(batch2[0].version(), Version::from_persisted(2));
    assert_eq!(batch2[1].version(), Version::from_persisted(3));
}

// =============================================================================
// version increments exactly by 1 per apply_event, even with many uncommitted
// =============================================================================

#[test]
fn version_increments_by_one_each_apply_with_many_uncommitted() {
    let mut agg = AggregateRoot::<TAgg>::new(TId(1));

    for i in 0..5 {
        let pre = agg.current_version();
        agg.apply_event(TEvent::Added(Added(format!("item-{i}"))));
        let post = agg.current_version();
        assert_eq!(
            post.as_u64(),
            pre.as_u64() + 1,
            "After apply_event #{i}, current_version should be exactly pre + 1"
        );
    }

    assert_eq!(agg.current_version(), Version::from_persisted(5));
    assert_eq!(agg.version(), Version::INITIAL); // persisted version unchanged

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 5);
    // Each event's version should be strictly sequential
    for (i, ve) in events.iter().enumerate() {
        assert_eq!(
            ve.version(),
            Version::from_persisted((i + 1) as u64),
            "Event {i} should have version {}",
            i + 1,
        );
    }
}

// =============================================================================
// Events<E> ref iteration — was untested
// =============================================================================

#[test]
fn events_ref_iteration() {
    let mut events = Events::new(TEvent::Added(Added("a".into())));
    events.add(TEvent::Removed(Removed));

    // Iterate by reference (non-consuming)
    let names: Vec<&str> = (&events).into_iter().map(|e| e.name()).collect();
    assert_eq!(names, vec!["Added", "Removed"]);

    // Original still usable
    assert_eq!(events.len(), 2);
}
