//! Property-based tests for macro-generated aggregates.
//!
//! Verifies that #[nexus::aggregate] produces code satisfying
//! the same algebraic properties as hand-written AggregateRoot.

use nexus::*;
use proptest::prelude::*;
use std::fmt;

// --- Domain ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PId([u8; 8]);
impl PId {
    fn new(id: u64) -> Self {
        Self(id.to_be_bytes())
    }
}
impl fmt::Display for PId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u64::from_be_bytes(self.0))
    }
}
impl AsRef<[u8]> for PId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Id for PId {
    const BYTE_LEN: usize = 8;
}

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
            CountEvent::Set(v) => self.value = *v as i64,
        }
        self
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
            let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(1));
            let mut ver = Version::INITIAL;
            for (i, e) in events.iter().enumerate() {
                let v = if i == 0 { Version::INITIAL } else { ver.next().expect("version") };
                agg.replay(v, e).unwrap();
                ver = v;
            }
            agg
        };

        let agg1 = make_agg(&raw_events);
        let agg2 = make_agg(&raw_events);

        prop_assert_eq!(agg1.state(), agg2.state());
        prop_assert_eq!(agg1.version(), agg2.version());
    }

    /// Property 2: Replay version equals event count
    ///
    /// After replaying N events, version == Some(N).
    #[test]
    fn prop_macro_version_equals_event_count(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(1));
        let n = events.len() as u64;

        let mut ver = Version::INITIAL;
        for (i, event) in events.iter().enumerate() {
            let v = if i == 0 { Version::INITIAL } else { ver.next().expect("version") };
            agg.replay(v, event).unwrap();
            ver = v;
        }

        prop_assert_eq!(agg.version(), Version::new(n));
    }

    // Properties 3 & 4 (apply_event leaves version None; replay-state ==
    // apply_event-state) were deleted as redundant: `apply_event`/`apply_events`
    // are now private to the `nexus` crate, and their in-isolation contracts are
    // covered by in-crate tests in `crates/nexus/src/aggregate.rs`
    // (`apply_event_accumulates_state_without_advancing_version` and
    // `commit_persisted_advances_version_and_folds_state_atomically`, which
    // asserts the commit-fold matches a replay of the same events).

    /// Property 5: Version tracks the cumulative replayed count across batches
    ///
    /// Replaying two batches in sequence leaves the version at the running
    /// total — the public rehydration path advances the version itself.
    #[test]
    fn prop_macro_version_tracks_cumulative_batches(
        batch1 in proptest::collection::vec(arb_event(), 1..20usize),
        batch2 in proptest::collection::vec(arb_event(), 1..20usize),
    ) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(1));
        let mut ver = Version::INITIAL;

        for (i, event) in batch1.iter().enumerate() {
            let v = if i == 0 { Version::INITIAL } else { ver.next().expect("version") };
            agg.replay(v, event).unwrap();
            ver = v;
        }
        let v1 = Version::new(batch1.len() as u64).expect("nonzero");
        prop_assert_eq!(agg.version(), Some(v1));

        for event in &batch2 {
            let v = ver.next().expect("version");
            agg.replay(v, event).unwrap();
            ver = v;
        }
        let v2 = Version::new((batch1.len() + batch2.len()) as u64).expect("nonzero");
        prop_assert_eq!(agg.version(), Some(v2));
    }

    /// Property 6: Fresh aggregate has no version
    ///
    /// A freshly created aggregate always has version None.
    #[test]
    fn prop_macro_fresh_no_version(_id in 0..1000u64) {
        let agg = AggregateRoot::<CounterAggregate>::new(PId::new(_id));
        prop_assert_eq!(agg.version(), None);
    }

    /// Property 7: AggregateRoot accessors reflect replayed events
    ///
    /// `id` is preserved, `version` tracks the replayed count, and `state`
    /// equals the net fold of every event.
    #[test]
    fn prop_macro_root_accessors_consistent(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(7));
        let mut ver = Version::INITIAL;
        for (i, event) in events.iter().enumerate() {
            let v = if i == 0 { Version::INITIAL } else { ver.next().expect("version") };
            agg.replay(v, event).unwrap();
            ver = v;
        }

        // Net effect of all events on the counter value.
        let expected_value: i64 = events.iter().fold(0i64, |acc, event| match event {
            CountEvent::Incremented => acc + 1,
            CountEvent::Decremented => acc - 1,
            CountEvent::Set(v) => *v as i64,
        });

        prop_assert_eq!(agg.id(), &PId::new(7));
        // `Version::new(0)` is `None`, matching a fresh aggregate with no events.
        prop_assert_eq!(agg.version(), Version::new(events.len() as u64));
        prop_assert_eq!(agg.state().value, expected_value);
    }
}
