//! Property-based tests for the kernel.
//!
//! These define algebraic properties that must hold for ALL random inputs,
//! not just specific examples. proptest runs each property hundreds of times
//! with different random data and automatically shrinks to minimal failing cases.
//!
//! Each property is a universal law about the kernel's behavior.

use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;
use nexus::kernel::version::VersionedEvent;
use proptest::prelude::*;
use std::fmt;

// =============================================================================
// Test domain — minimal types for property testing
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PId(u64);
impl fmt::Display for PId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p-{}", self.0)
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
            CountEvent::Incremented => "Incremented",
            CountEvent::Decremented => "Decremented",
            CountEvent::Set(_) => "Set",
        }
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
struct CountState {
    value: i64,
}
impl AggregateState for CountState {
    type Event = CountEvent;
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

#[derive(Debug)]
struct CountAgg;
#[derive(Debug, thiserror::Error)]
#[error("count error")]
struct CountErr;
impl Aggregate for CountAgg {
    type State = CountState;
    type Error = CountErr;
    type Id = PId;
}

// =============================================================================
// Strategies — generate random valid data
// =============================================================================

/// Generate a random CountEvent
fn arb_event() -> impl Strategy<Value = CountEvent> {
    prop_oneof![
        Just(CountEvent::Incremented),
        Just(CountEvent::Decremented),
        (0..1000u64).prop_map(CountEvent::Set),
    ]
}

/// Generate a valid versioned event sequence (contiguous versions starting at 1)
fn arb_versioned_events(max_len: usize) -> impl Strategy<Value = Vec<VersionedEvent<CountEvent>>> {
    proptest::collection::vec(arb_event(), 0..max_len).prop_map(|events| {
        events
            .into_iter()
            .enumerate()
            .map(|(i, event)| VersionedEvent {
                version: Version::from((i + 1) as u64),
                event,
            })
            .collect()
    })
}

// =============================================================================
// Properties
// =============================================================================

proptest! {
    /// Property 1: Replay determinism
    ///
    /// Loading the same event sequence twice must produce identical state.
    /// This is fundamental to event sourcing — state is a pure function of events.
    #[test]
    fn prop_replay_is_deterministic(events in arb_versioned_events(50)) {
        let agg1 = AggregateRoot::<CountAgg>::load_from_events(PId(1), events.clone()).unwrap();
        let agg2 = AggregateRoot::<CountAgg>::load_from_events(PId(1), events).unwrap();

        prop_assert_eq!(agg1.state(), agg2.state());
        prop_assert_eq!(agg1.version(), agg2.version());
    }

    /// Property 2: Version consistency
    ///
    /// After applying N events, current_version must be INITIAL + N.
    /// Version is a strict counter — no gaps, no jumps.
    #[test]
    fn prop_version_equals_event_count(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        let n = events.len() as u64;

        for event in events {
            agg.apply_event(event);
        }

        prop_assert_eq!(agg.current_version(), Version::from(n));
    }

    /// Property 3: Uncommitted count matches version delta
    ///
    /// current_version() - version() == number of uncommitted events.
    /// This is the invariant that ties version tracking to event tracking.
    #[test]
    fn prop_uncommitted_count_matches_version_delta(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply_event(event);
        }

        let uncommitted = agg.take_uncommitted_events();
        let delta = agg.current_version().as_u64() - agg.version().as_u64();
        // After take, delta should be 0 (all events accounted for)
        prop_assert_eq!(delta, 0);
        // And version should equal what current_version was before take
        prop_assert_eq!(agg.version().as_u64(), uncommitted.len() as u64);
    }

    /// Property 4: Rehydrate-apply equivalence
    ///
    /// Loading from versioned events produces the same state as
    /// creating a new aggregate and applying the raw events.
    /// This proves load_from_events is equivalent to sequential apply.
    #[test]
    fn prop_rehydrate_equals_sequential_apply(events in arb_versioned_events(50)) {
        // Path A: load_from_events
        let agg_loaded = AggregateRoot::<CountAgg>::load_from_events(
            PId(1),
            events.clone(),
        ).unwrap();

        // Path B: new() + apply_events()
        let mut agg_applied = AggregateRoot::<CountAgg>::new(PId(1));
        for ve in &events {
            agg_applied.apply_event(ve.event.clone());
        }

        prop_assert_eq!(agg_loaded.state(), agg_applied.state());
    }

    /// Property 5: Version sequence integrity
    ///
    /// Every event in take_uncommitted_events() has strictly increasing
    /// version numbers with no gaps.
    #[test]
    fn prop_uncommitted_versions_are_contiguous(events in proptest::collection::vec(arb_event(), 1..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply_event(event);
        }

        let uncommitted = agg.take_uncommitted_events();
        for (i, ve) in uncommitted.iter().enumerate() {
            let expected = Version::from((i + 1) as u64);
            prop_assert_eq!(
                ve.version, expected,
                "Event at index {} has version {:?}, expected {:?}",
                i, ve.version, expected,
            );
        }
    }

    /// Property 6: Version gap rejection
    ///
    /// load_from_events must reject ANY sequence with a version gap.
    /// We generate a valid sequence then corrupt one version.
    #[test]
    fn prop_rejects_any_version_gap(
        events in arb_versioned_events(50).prop_filter(
            "need at least 2 events to create a gap",
            |events| events.len() >= 2,
        ),
        corrupt_idx in 1..50usize,
    ) {
        let corrupt_idx = corrupt_idx % (events.len() - 1) + 1; // ensure valid index > 0
        let mut corrupted = events;
        // Add 1 to create a gap (skip a version)
        corrupted[corrupt_idx].version = Version::from(
            corrupted[corrupt_idx].version.as_u64() + 1
        );

        let result = AggregateRoot::<CountAgg>::load_from_events(PId(1), corrupted);
        prop_assert!(result.is_err(), "Should reject sequence with version gap");
    }

    /// Property 7: Take is idempotent
    ///
    /// Calling take_uncommitted_events twice in a row:
    /// first call returns all events, second returns empty.
    #[test]
    fn prop_take_twice_second_is_empty(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply_event(event);
        }

        let _first = agg.take_uncommitted_events();
        let second = agg.take_uncommitted_events();
        prop_assert!(second.is_empty(), "Second take should return empty");
    }

    /// Property 8: Version continuity across take cycles
    ///
    /// After take + more events, new event versions continue from
    /// where the previous batch ended. No version reuse.
    #[test]
    fn prop_version_continuity_across_takes(
        batch1 in proptest::collection::vec(arb_event(), 1..20),
        batch2 in proptest::collection::vec(arb_event(), 1..20),
    ) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));

        for event in &batch1 {
            agg.apply_event(event.clone());
        }
        let taken1 = agg.take_uncommitted_events();
        let last_v1 = taken1.last().unwrap().version;

        for event in &batch2 {
            agg.apply_event(event.clone());
        }
        let taken2 = agg.take_uncommitted_events();
        let first_v2 = taken2.first().unwrap().version;

        prop_assert_eq!(
            first_v2,
            last_v1.next(),
            "Second batch must continue from first batch's last version"
        );
    }
}
