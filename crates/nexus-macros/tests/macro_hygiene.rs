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

#[derive(Default, Debug, Clone)]
struct HState {
    count: u64,
}
impl AggregateState for HState {
    type Event = HEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, _: &HEvent) -> Self {
        self.count += 1;
        self
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
    agg.root_mut().apply_event(&HEvent::A);
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

    first.root_mut().apply_event(&HEvent::A);
    first.root_mut().apply_event(&HEvent::A);
    second.root_mut().apply_event(&HEvent::A);

    assert_eq!(first.state().count, 2);
    assert_eq!(second.state().count, 1);
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

    #[derive(Default, Debug, Clone)]
    struct LocalState {
        ticks: u32,
    }
    impl AggregateState for LocalState {
        type Event = LocalEvent;
        fn initial() -> Self {
            Self::default()
        }
        fn apply(mut self, _: &LocalEvent) -> Self {
            self.ticks += 1;
            self
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct LocalError;

    #[nexus::aggregate(state = LocalState, error = LocalError, id = HId)]
    struct LocalAggregate;

    let mut agg = LocalAggregate::new(HId(1));
    agg.root_mut().apply_event(&LocalEvent::Tick);
    agg.root_mut().apply_event(&LocalEvent::Tick);
    agg.root_mut().apply_event(&LocalEvent::Tick);
    assert_eq!(agg.state().ticks, 3);
}

// =============================================================================
// Test: user has their own trait called `Aggregate` in scope — no ambiguity
// because macro uses fully qualified ::nexus:: paths
// =============================================================================

mod user_has_own_aggregate_trait {
    use super::*;

    // User-defined trait with the same name
    trait Aggregate {
        fn custom_method(&self) -> &str;
    }

    #[nexus::aggregate(state = HState, error = HError, id = HId)]
    struct MyAgg;

    // User can still implement their own `Aggregate` on the type
    impl Aggregate for MyAgg {
        fn custom_method(&self) -> &str {
            "custom"
        }
    }

    #[test]
    fn user_aggregate_trait_no_ambiguity() {
        let mut agg = MyAgg::new(HId(1));
        agg.root_mut().apply_event(&HEvent::A); // from nexus::AggregateEntity
        assert_eq!(agg.custom_method(), "custom"); // from user's Aggregate
        assert_eq!(agg.state().count, 1); // from nexus::AggregateEntity
    }
}

// =============================================================================
// Test: user has a type called `AggregateRoot` — no conflict because
// macro uses ::nexus::AggregateRoot (fully qualified)
// =============================================================================

mod user_has_own_aggregate_root_type {
    use super::*;

    // User-defined type with the same name
    struct AggregateRoot {
        data: String,
    }

    #[nexus::aggregate(state = HState, error = HError, id = HId)]
    struct MyAgg;

    #[test]
    fn user_aggregate_root_type_no_conflict() {
        let user_root = AggregateRoot {
            data: "user".into(),
        };
        let mut agg = MyAgg::new(HId(1));
        agg.root_mut().apply_event(&HEvent::A);

        assert_eq!(user_root.data, "user");
        assert_eq!(agg.state().count, 1);
    }
}

// =============================================================================
// Test: user has functions called `state`, `version`, `id` — no conflict
// because those come from AggregateEntity trait methods, not free functions
// =============================================================================

fn state() -> &'static str {
    "free function state"
}

fn version() -> u32 {
    42
}

fn id() -> u64 {
    999
}

#[nexus::aggregate(state = HState, error = HError, id = HId)]
struct FnConflictAgg;

#[test]
fn user_functions_named_like_trait_methods() {
    let mut agg = FnConflictAgg::new(HId(1));
    agg.root_mut().apply_event(&HEvent::A);

    // Trait methods on agg
    assert_eq!(agg.state().count, 1);
    // Version is None because apply_event does not advance version
    assert_eq!(agg.version(), None);
    assert_eq!(agg.id(), &HId(1));

    // Free functions still accessible
    assert_eq!(state(), "free function state");
    assert_eq!(version(), 42);
    assert_eq!(id(), 999);
}

// =============================================================================
// Test: DomainEvent variant name same as a type in scope
// =============================================================================

struct Created; // user type named `Created`

#[derive(Debug, Clone, DomainEvent)]
#[allow(dead_code, reason = "test-only event type for macro hygiene tests")]
enum ConflictEvent {
    Created(CreatedPayload), // variant also named `Created`
    Deleted,
}

#[derive(Debug, Clone)]
#[allow(dead_code, reason = "test-only payload type for macro hygiene tests")]
struct CreatedPayload {
    name: String,
}

#[test]
fn domain_event_variant_name_same_as_type_in_scope() {
    let _user_type = Created; // user's type
    let event = ConflictEvent::Created(CreatedPayload {
        name: "test".into(),
    });
    assert_eq!(event.name(), "Created"); // from DomainEvent derive
}
