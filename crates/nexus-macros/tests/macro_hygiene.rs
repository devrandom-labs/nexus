//! Macro hygiene tests — verify macros don't clash with user names.

use nexus::*;
use std::fmt;

// Shared test domain
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct HId(u64);
impl fmt::Display for HId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for HId {}

#[derive(Debug, Clone)]
enum HEvent {
    A,
}
impl Message for HEvent {}
impl DomainEvent for HEvent {
    fn name(&self) -> &'static str {
        "A"
    }
}

#[derive(Default, Debug)]
struct HState {
    count: u64,
}
impl AggregateState for HState {
    type Event = HEvent;
    fn apply(&mut self, _: &HEvent) {
        self.count += 1;
    }
    fn name(&self) -> &'static str {
        "H"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct HError;

// =============================================================================
// Test: user has local variable called `root` — no conflict
// =============================================================================

#[nexus::aggregate(state = HState, error = HError, id = HId)]
struct AggWithRoot;

#[test]
fn user_variable_named_root_no_conflict() {
    let root = "I am a local variable named root";
    let mut agg = AggWithRoot::new(HId(1));
    agg.apply_event(HEvent::A);
    assert_eq!(agg.state().count, 1);
    assert_eq!(root, "I am a local variable named root");
}

// =============================================================================
// Test: two aggregates in the same module — no interference
// =============================================================================

#[nexus::aggregate(state = HState, error = HError, id = HId)]
struct FirstAggregate;

#[nexus::aggregate(state = HState, error = HError, id = HId)]
struct SecondAggregate;

#[test]
fn two_aggregates_same_module_no_interference() {
    let mut first = FirstAggregate::new(HId(1));
    let mut second = SecondAggregate::new(HId(2));

    first.apply_event(HEvent::A);
    first.apply_event(HEvent::A);
    second.apply_event(HEvent::A);

    assert_eq!(first.state().count, 2);
    assert_eq!(second.state().count, 1);

    // They have independent versions
    assert_eq!(first.current_version(), Version::from_persisted(2));
    assert_eq!(second.current_version(), Version::from_persisted(1));
}

// =============================================================================
// Test: aggregate defined inside a function body
// =============================================================================

#[test]
fn aggregate_inside_function_body() {
    #[derive(Debug, Clone)]
    enum LocalEvent {
        Tick,
    }
    impl Message for LocalEvent {}
    impl DomainEvent for LocalEvent {
        fn name(&self) -> &'static str {
            "Tick"
        }
    }

    #[derive(Default, Debug)]
    struct LocalState {
        ticks: u32,
    }
    impl AggregateState for LocalState {
        type Event = LocalEvent;
        fn apply(&mut self, _: &LocalEvent) {
            self.ticks += 1;
        }
        fn name(&self) -> &'static str {
            "Local"
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct LocalError;

    #[nexus::aggregate(state = LocalState, error = LocalError, id = HId)]
    struct LocalAggregate;

    let mut agg = LocalAggregate::new(HId(1));
    agg.apply_event(LocalEvent::Tick);
    agg.apply_event(LocalEvent::Tick);
    agg.apply_event(LocalEvent::Tick);
    assert_eq!(agg.state().ticks, 3);
}
