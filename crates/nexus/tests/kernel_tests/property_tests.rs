//! Property-based tests for the kernel.
//!
//! These define algebraic properties that must hold for ALL random inputs,
//! not just specific examples. proptest runs each property hundreds of times
//! with different random data and automatically shrinks to minimal failing cases.
//!
//! Each property is a universal law about the kernel's behavior.
//!
//! Skipped under Miri: proptest uses filesystem I/O (getcwd) which Miri's
//! isolation blocks, and interpreting hundreds of iterations is impractical.
//! Miri and proptest serve different purposes — Miri catches UB, proptest
//! catches logic bugs.

#![cfg(not(miri))]

use nexus::AggregateRoot;
use nexus::*;
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
    fn apply(mut self, event: &CountEvent) -> Self {
        match event {
            CountEvent::Incremented => self.value += 1,
            CountEvent::Decremented => self.value -= 1,
            CountEvent::Set(v) => self.value = (*v).cast_signed(),
        }
        self
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

/// Generate a random `CountEvent`
fn arb_event() -> impl Strategy<Value = CountEvent> {
    prop_oneof![
        Just(CountEvent::Incremented),
        Just(CountEvent::Decremented),
        (0..1000u64).prop_map(CountEvent::Set),
    ]
}

/// Generate a valid versioned event sequence (contiguous versions starting at 1)
fn arb_versioned_events(max_len: usize) -> impl Strategy<Value = Vec<(Version, CountEvent)>> {
    proptest::collection::vec(arb_event(), 0..max_len).prop_map(|events| {
        events
            .into_iter()
            .enumerate()
            .map(|(i, event)| (Version::from_persisted((i + 1) as u64), event))
            .collect()
    })
}

// =============================================================================
// Properties
// =============================================================================

proptest! {
    /// Property 1: Replay determinism
    ///
    /// Replaying the same event sequence twice must produce identical state.
    /// This is fundamental to event sourcing — state is a pure function of events.
    /// We test by replaying the same raw events via two different aggregate instances.
    #[test]
    fn prop_replay_is_deterministic(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        let replay_all = |events: &[CountEvent]| -> AggregateRoot<CountAgg> {
            let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
            for (i, e) in events.iter().enumerate() {
                agg.replay(Version::from_persisted((i + 1) as u64), e).unwrap();
            }
            agg
        };

        let agg1 = replay_all(&raw_events);
        let agg2 = replay_all(&raw_events);

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
            agg.apply(event);
        }

        prop_assert_eq!(agg.current_version(), Version::from_persisted(n));
    }

    /// Property 3: Uncommitted count matches version delta
    ///
    /// current_version() - version() == number of uncommitted events.
    /// This is the invariant that ties version tracking to event tracking.
    #[test]
    fn prop_uncommitted_count_matches_version_delta(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply(event);
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
    /// Replaying versioned events produces the same state as
    /// creating a new aggregate and applying the raw events.
    /// This proves replay is equivalent to sequential apply.
    #[test]
    fn prop_rehydrate_equals_sequential_apply(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        // Path A: new() + replay()
        let mut agg_replayed = AggregateRoot::<CountAgg>::new(PId(1));
        for (i, e) in raw_events.iter().enumerate() {
            agg_replayed.replay(Version::from_persisted((i + 1) as u64), e).unwrap();
        }

        // Path B: new() + apply()
        let mut agg_applied = AggregateRoot::<CountAgg>::new(PId(1));
        for event in &raw_events {
            agg_applied.apply(event.clone());
        }

        prop_assert_eq!(agg_replayed.state(), agg_applied.state());
    }

    /// Property 5: Version sequence integrity
    ///
    /// Every event in take_uncommitted_events() has strictly increasing
    /// version numbers with no gaps.
    #[test]
    fn prop_uncommitted_versions_are_contiguous(events in proptest::collection::vec(arb_event(), 1..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply(event);
        }

        let uncommitted = agg.take_uncommitted_events();
        for (i, ve) in uncommitted.iter().enumerate() {
            let expected = Version::from_persisted((i + 1) as u64);
            prop_assert_eq!(
                ve.version(), expected,
                "Event at index {} has version {:?}, expected {:?}",
                i, ve.version(), expected,
            );
        }
    }

    /// Property 6: Version gap rejection
    ///
    /// replay must reject ANY sequence with a version gap.
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
        let (v, ref e) = corrupted[corrupt_idx];
        corrupted[corrupt_idx] = (Version::from_persisted(v.as_u64() + 1), e.clone());

        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        let mut found_error = false;
        for (version, event) in &corrupted {
            if agg.replay(*version, event).is_err() {
                found_error = true;
                break;
            }
        }
        prop_assert!(found_error, "Should reject sequence with version gap");
    }

    /// Property 7: Take is idempotent
    ///
    /// Calling take_uncommitted_events twice in a row:
    /// first call returns all events, second returns empty.
    #[test]
    fn prop_take_twice_second_is_empty(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        for event in events {
            agg.apply(event);
        }

        let _first = agg.take_uncommitted_events();
        let second = agg.take_uncommitted_events();
        prop_assert!(second.is_empty(), "Second take should return empty");
    }

    /// Property 8: State is a pure function of events
    ///
    /// Building state from the same events via different paths
    /// (apply vs replay vs apply-take-replay) must all converge.
    /// This proves apply() behaves as a pure state transition.
    #[test]
    fn prop_state_is_pure_function_of_events(events in proptest::collection::vec(arb_event(), 1..50)) {
        // Path A: apply all
        let mut a = AggregateRoot::<CountAgg>::new(PId(1));
        for e in &events { a.apply(e.clone()); }

        // Path B: replay all
        let mut b = AggregateRoot::<CountAgg>::new(PId(1));
        for (i, e) in events.iter().enumerate() {
            b.replay(Version::from_persisted((i + 1) as u64), e).unwrap();
        }

        // Path C: apply first half, take, then replay second half
        let mid = events.len() / 2;
        let mut c = AggregateRoot::<CountAgg>::new(PId(1));
        for e in &events[..mid] { c.apply(e.clone()); }
        let _ = c.take_uncommitted_events();
        for (i, e) in events[mid..].iter().enumerate() {
            let v = (mid + i + 1) as u64;
            c.replay(Version::from_persisted(v), e).unwrap();
        }

        prop_assert_eq!(a.state(), b.state(), "apply vs replay diverged");
        prop_assert_eq!(b.state(), c.state(), "replay vs apply-take-replay diverged");
    }

    /// Property 9: Version continuity across take cycles
    ///
    /// After take + more events, new event versions continue from
    /// where the previous batch ended. No version reuse.
    #[allow(clippy::expect_used, reason = "property test needs unwrap")]
    #[test]
    fn prop_version_continuity_across_takes(
        batch1 in proptest::collection::vec(arb_event(), 1..20),
        batch2 in proptest::collection::vec(arb_event(), 1..20),
    ) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));

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

        prop_assert_eq!(
            first_v2,
            last_v1.next(),
            "Second batch must continue from first batch's last version"
        );
    }
}
