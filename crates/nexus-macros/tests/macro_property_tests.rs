//! Property-based tests for macro-generated aggregates.
//!
//! Verifies that #[nexus::aggregate] produces code satisfying
//! the same algebraic properties as hand-written AggregateRoot.

use nexus::*;
use proptest::prelude::*;
use std::fmt;

// --- Domain ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PId(u64);
impl fmt::Display for PId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for PId {}

#[derive(Debug, Clone, PartialEq)]
enum CountEvent {
    Incremented,
    Decremented,
    Set(u64),
}
impl Message for CountEvent {}
impl DomainEvent for CountEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Incremented => "Incremented",
            Self::Decremented => "Decremented",
            Self::Set(_) => "Set",
        }
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
struct CountState {
    value: i64,
}
impl AggregateState for CountState {
    type Event = CountEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(&mut self, event: &CountEvent) {
        match event {
            CountEvent::Incremented => self.value += 1,
            CountEvent::Decremented => self.value -= 1,
            CountEvent::Set(v) => self.value = *v as i64,
        }
    }
    fn name(&self) -> &'static str {
        "Counter"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("counter error")]
struct CountError;

// --- Macro-generated aggregate ---

#[nexus::aggregate(state = CountState, error = CountError, id = PId)]
struct CounterAggregate;

// --- Strategies ---

fn arb_event() -> impl Strategy<Value = CountEvent> {
    prop_oneof![
        Just(CountEvent::Incremented),
        Just(CountEvent::Decremented),
        (0..1000u64).prop_map(CountEvent::Set),
    ]
}

// --- Properties ---

proptest! {
    /// Property 1: Replay determinism
    ///
    /// Replaying the same events twice produces identical state.
    #[test]
    fn prop_macro_replay_deterministic(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        let make_agg = |events: &[CountEvent]| {
            let mut agg = CounterAggregate::new(PId(1));
            for (i, e) in events.iter().enumerate() {
                agg.root_mut().replay(Version::from_persisted((i + 1) as u64), e).unwrap();
            }
            agg
        };

        let agg1 = make_agg(&raw_events);
        let agg2 = make_agg(&raw_events);

        prop_assert_eq!(agg1.state(), agg2.state());
        prop_assert_eq!(agg1.version(), agg2.version());
    }

    /// Property 2: Version equals event count
    ///
    /// After applying N events, current_version == N.
    #[test]
    fn prop_macro_version_equals_event_count(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = CounterAggregate::new(PId(1));
        let n = events.len() as u64;

        for event in events {
            agg.apply(event);
        }

        prop_assert_eq!(agg.current_version(), Version::from_persisted(n));
    }

    /// Property 3: Uncommitted count matches version delta
    ///
    /// After take, version == current_version (delta is 0).
    #[test]
    fn prop_macro_uncommitted_matches_delta(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = CounterAggregate::new(PId(1));
        for event in events {
            agg.apply(event);
        }

        let uncommitted = agg.take_uncommitted_events();
        prop_assert_eq!(agg.version(), agg.current_version());
        prop_assert_eq!(agg.version().as_u64(), uncommitted.len() as u64);
    }

    /// Property 4: Rehydrate-apply equivalence
    ///
    /// replay produces the same state as new() + apply().
    #[test]
    fn prop_macro_rehydrate_equals_apply(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        // Path A: new() + replay()
        let mut agg_replayed = CounterAggregate::new(PId(1));
        for (i, e) in raw_events.iter().enumerate() {
            agg_replayed.root_mut().replay(Version::from_persisted((i + 1) as u64), e).unwrap();
        }

        // Path B: new() + apply()
        let mut agg_applied = CounterAggregate::new(PId(1));
        for event in &raw_events {
            agg_applied.apply(event.clone());
        }

        prop_assert_eq!(agg_replayed.state(), agg_applied.state());
    }

    /// Property 5: Version continuity across take cycles
    ///
    /// After take + more events, versions continue — no gaps, no reuse.
    #[test]
    fn prop_macro_version_continuity(
        batch1 in proptest::collection::vec(arb_event(), 1..20),
        batch2 in proptest::collection::vec(arb_event(), 1..20),
    ) {
        let mut agg = CounterAggregate::new(PId(1));

        for event in &batch1 {
            agg.apply(event.clone());
        }
        let taken1 = agg.take_uncommitted_events();
        let last_v1 = taken1.last().unwrap().version();

        for event in &batch2 {
            agg.apply(event.clone());
        }
        let taken2 = agg.take_uncommitted_events();
        let first_v2 = taken2.first().unwrap().version();

        prop_assert_eq!(first_v2, last_v1.next());
    }

    /// Property 6: Take is idempotent
    ///
    /// Second take always returns empty.
    #[test]
    fn prop_macro_take_idempotent(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = CounterAggregate::new(PId(1));
        for event in events {
            agg.apply(event);
        }

        let _first = agg.take_uncommitted_events();
        let second = agg.take_uncommitted_events();
        prop_assert!(second.is_empty());
    }

    /// Property 7: AggregateEntity methods match AggregateRoot methods
    ///
    /// The trait default methods must produce identical results to
    /// calling the methods directly on the inner AggregateRoot.
    #[test]
    fn prop_macro_entity_matches_root(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = CounterAggregate::new(PId(1));
        for event in events {
            agg.apply(event);
        }

        // AggregateEntity methods (via trait defaults)
        let entity_version = agg.version();
        let entity_current = agg.current_version();
        let entity_state = agg.state().clone();
        let entity_id = agg.id().clone();

        // AggregateRoot methods (via root())
        let root_version = agg.root().version();
        let root_current = agg.root().current_version();
        let root_state = agg.root().state().clone();
        let root_id = agg.root().id().clone();

        prop_assert_eq!(entity_version, root_version);
        prop_assert_eq!(entity_current, root_current);
        prop_assert_eq!(entity_state, root_state);
        prop_assert_eq!(entity_id, root_id);
    }
}
