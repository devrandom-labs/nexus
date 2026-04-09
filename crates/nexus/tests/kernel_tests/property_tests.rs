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

/// Helper: replay events into a fresh aggregate, returns the aggregate.
fn replay_events(events: &[CountEvent]) -> AggregateRoot<CountAgg> {
    let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
    for (i, e) in events.iter().enumerate() {
        let v = Version::new((i + 1) as u64).unwrap();
        agg.replay(v, e).unwrap();
    }
    agg
}

/// Helper: apply events via `apply_events` (simulating post-persistence state update).
fn apply_events_to(events: &[CountEvent]) -> AggregateRoot<CountAgg> {
    let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
    for (i, e) in events.iter().enumerate() {
        let v = Version::new((i + 1) as u64).unwrap();
        let batch: Events<_, 0> = Events::new(e.clone());
        agg.advance_version(v);
        agg.apply_events(&batch);
    }
    agg
}

// =============================================================================
// Properties
// =============================================================================

proptest! {
    /// Property 1: Replay determinism
    ///
    /// Replaying the same event sequence twice must produce identical state.
    /// This is fundamental to event sourcing — state is a pure function of events.
    #[test]
    fn prop_replay_is_deterministic(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        let agg1 = replay_events(&raw_events);
        let agg2 = replay_events(&raw_events);

        prop_assert_eq!(agg1.state(), agg2.state());
        prop_assert_eq!(agg1.version(), agg2.version());
    }

    /// Property 2: Replay version equals event count
    ///
    /// After replaying N events, version must be Some(N).
    /// After replaying 0 events, version must be None.
    #[test]
    fn prop_replay_version_equals_event_count(events in proptest::collection::vec(arb_event(), 0..50)) {
        let agg = replay_events(&events);
        let n = events.len() as u64;

        if n == 0 {
            prop_assert_eq!(agg.version(), None);
        } else {
            prop_assert_eq!(agg.version(), Version::new(n));
        }
    }

    /// Property 3: Replay-apply_events equivalence
    ///
    /// Replaying events and applying them via apply_events must produce
    /// the same state. Both paths are pure state transitions.
    #[test]
    fn prop_replay_equals_apply_events(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        let replayed = replay_events(&raw_events);
        let applied = apply_events_to(&raw_events);

        prop_assert_eq!(replayed.state(), applied.state());
    }

    /// Property 4: Version gap rejection
    ///
    /// replay must reject ANY sequence with a version gap.
    /// We generate a valid sequence then corrupt one version.
    #[test]
    fn prop_rejects_any_version_gap(
        events in proptest::collection::vec(arb_event(), 2..50),
        corrupt_idx in 1..50usize,
    ) {
        let corrupt_idx = corrupt_idx % (events.len() - 1) + 1; // ensure valid index > 0

        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        let mut found_error = false;
        for (i, event) in events.iter().enumerate() {
            // At corrupt_idx, skip a version to create a gap
            let v = if i >= corrupt_idx { i + 2 } else { i + 1 };
            let version = Version::new(v as u64).unwrap();
            if agg.replay(version, event).is_err() {
                found_error = true;
                break;
            }
        }
        prop_assert!(found_error, "Should reject sequence with version gap");
    }

    /// Property 5: State is a pure function of events
    ///
    /// Replaying the same events in two separate aggregates must converge.
    /// Split at any midpoint and verify both halves combined equal the whole.
    #[test]
    fn prop_state_is_pure_function_of_events(
        events in proptest::collection::vec(arb_event(), 1..50),
        split_pct in 0..100u32,
    ) {
        // Full replay
        let full = replay_events(&events);

        // Split: replay first half, then continue with second half
        let mid = (events.len() * split_pct as usize) / 100;
        let mut split = replay_events(&events[..mid]);
        for (i, e) in events[mid..].iter().enumerate() {
            let v = (mid + i + 1) as u64;
            split.replay(Version::new(v).unwrap(), e).unwrap();
        }

        prop_assert_eq!(full.state(), split.state(), "full vs split diverged");
        prop_assert_eq!(full.version(), split.version(), "full vs split version diverged");
    }

    /// Property 6: Replay duplicate version rejection
    ///
    /// After replaying N events, replaying version N again must fail.
    #[test]
    fn prop_replay_rejects_duplicate_version(events in proptest::collection::vec(arb_event(), 1..50)) {
        let mut agg = replay_events(&events);
        let last_version = agg.version().unwrap();

        let result = agg.replay(last_version, &CountEvent::Incremented);
        prop_assert!(result.is_err(), "Should reject duplicate version");
    }

    /// Property 7: Version::new roundtrip
    ///
    /// For any non-zero u64, Version::new(v).unwrap().as_u64() == v.
    /// For zero, Version::new returns None.
    #[test]
    fn prop_version_new_roundtrip(v in any::<u64>()) {
        let version = Version::new(v);
        if v == 0 {
            prop_assert!(version.is_none(), "Version::new(0) must return None");
        } else {
            let version = version.unwrap();
            prop_assert_eq!(version.as_u64(), v, "Version::new/as_u64 roundtrip failed");
        }
    }

    /// Property 8: Version::next is strictly monotonic
    ///
    /// For any version v < MAX, v.next() > v.
    #[test]
    fn prop_version_next_is_monotonic(v in 1..u64::MAX) {
        let version = Version::new(v).unwrap();
        let next = version.next().unwrap();
        prop_assert!(next > version, "next() must be strictly greater");
        prop_assert_eq!(next.as_u64(), v + 1, "next() must increment by exactly 1");
    }

    /// Property 9: advance_version + apply_events is idempotent on version
    ///
    /// Calling advance_version with the same version is a no-op on version.
    #[test]
    fn prop_advance_version_is_idempotent(v in 1..=u64::MAX) {
        let mut agg = AggregateRoot::<CountAgg>::new(PId(1));
        let version = Version::new(v).unwrap();

        agg.advance_version(version);
        prop_assert_eq!(agg.version(), Some(version));

        // Calling again with the same version doesn't change it
        agg.advance_version(version);
        prop_assert_eq!(agg.version(), Some(version));
    }
}
