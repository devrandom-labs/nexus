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

    /// Property 3: apply_event updates state without advancing version
    ///
    /// apply_event is for post-persistence sync, so version stays None.
    #[test]
    fn prop_macro_apply_event_no_version(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(1));
        for event in &events {
            agg.apply_event(event);
        }

        // Version should still be None — apply_event does not advance version
        prop_assert_eq!(agg.version(), None);
    }

    /// Property 4: Rehydrate-apply equivalence
    ///
    /// replay produces the same state as new() + apply_event().
    #[test]
    fn prop_macro_rehydrate_equals_apply(raw_events in proptest::collection::vec(arb_event(), 0..50)) {
        // Path A: new() + replay()
        let mut agg_replayed = AggregateRoot::<CounterAggregate>::new(PId::new(1));
        let mut ver = Version::INITIAL;
        for (i, e) in raw_events.iter().enumerate() {
            let v = if i == 0 { Version::INITIAL } else { ver.next().expect("version") };
            agg_replayed.replay(v, e).unwrap();
            ver = v;
        }

        // Path B: new() + apply_event()
        let mut agg_applied = AggregateRoot::<CounterAggregate>::new(PId::new(1));
        for event in &raw_events {
            agg_applied.apply_event(event);
        }

        prop_assert_eq!(agg_replayed.state(), agg_applied.state());
    }

    /// Property 5: Version advances via advance_version
    ///
    /// After advance_version, version reflects the new value.
    #[test]
    fn prop_macro_advance_version(
        batch1 in proptest::collection::vec(arb_event(), 1..20usize),
        batch2 in proptest::collection::vec(arb_event(), 1..20usize),
    ) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(1));

        // Apply batch1 events and advance version
        for event in &batch1 {
            agg.apply_event(event);
        }
        let v1 = Version::new(batch1.len() as u64).expect("nonzero");
        agg.advance_version(v1);
        prop_assert_eq!(agg.version(), Some(v1));

        // Apply batch2 events and advance version
        for event in &batch2 {
            agg.apply_event(event);
        }
        let v2 = Version::new((batch1.len() + batch2.len()) as u64).expect("nonzero");
        agg.advance_version(v2);
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

    /// Property 7: AggregateRoot accessors reflect applied events
    ///
    /// `id` is preserved, `version` stays None under `apply_event`, and
    /// `state` equals the net fold of every applied event.
    #[test]
    fn prop_macro_root_accessors_consistent(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut agg = AggregateRoot::<CounterAggregate>::new(PId::new(7));
        for event in &events {
            agg.apply_event(event);
        }

        // Net effect of all events on the counter value.
        let expected_value: i64 = events.iter().fold(0i64, |acc, event| match event {
            CountEvent::Incremented => acc + 1,
            CountEvent::Decremented => acc - 1,
            CountEvent::Set(v) => *v as i64,
        });

        prop_assert_eq!(agg.id(), &PId::new(7));
        prop_assert_eq!(agg.version(), None);
        prop_assert_eq!(agg.state().value, expected_value);
    }
}
