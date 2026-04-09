//! Adversarial input tests — weird edge cases the macro must handle.

use nexus::*;
use std::fmt;

// Shared helpers
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AId(u64);
impl fmt::Display for AId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for AId {}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct AError;

// =============================================================================
// Test: single-variant event enum
// =============================================================================

#[derive(Debug, Clone, DomainEvent)]
enum SingleEvent {
    Only,
}

#[derive(Default, Debug, Clone)]
struct SingleState {
    triggered: bool,
}
impl AggregateState for SingleState {
    type Event = SingleEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &SingleEvent) -> Self {
        match event {
            SingleEvent::Only => self.triggered = true,
        }
        self
    }
}

#[nexus::aggregate(state = SingleState, error = AError, id = AId)]
struct SingleAggregate;

#[test]
fn single_variant_event_enum() {
    let mut agg = SingleAggregate::new(AId(1));
    agg.root_mut().apply_event(&SingleEvent::Only);
    assert!(agg.state().triggered);
}

// =============================================================================
// Test: event enum with many variants (stress test variant arm generation)
// =============================================================================

#[derive(Debug, Clone, DomainEvent)]
#[allow(
    dead_code,
    reason = "test-only event type — variants exist to stress-test macro variant arm generation"
)]
enum ManyEvent {
    V1,
    V2(u8),
    V3(String),
    V4 { x: i32 },
    V5 { a: String, b: u64 },
    V6,
    V7(Vec<u8>),
    V8,
    V9,
    V10,
}

#[test]
fn many_variant_event_names() {
    assert_eq!(ManyEvent::V1.name(), "V1");
    assert_eq!(ManyEvent::V2(0).name(), "V2");
    assert_eq!(ManyEvent::V3(String::new()).name(), "V3");
    assert_eq!(ManyEvent::V4 { x: 0 }.name(), "V4");
    assert_eq!(
        ManyEvent::V5 {
            a: String::new(),
            b: 0
        }
        .name(),
        "V5"
    );
    assert_eq!(ManyEvent::V10.name(), "V10");
}

// =============================================================================
// Test: aggregate with very long type names
// =============================================================================

#[derive(Debug, Clone, DomainEvent)]
enum VeryLongEventNameThatShouldStillWorkCorrectlyWithTheMacro {
    SomethingHappenedWithAnExtremelyDescriptiveNameThatGoesOnAndOn(String),
}

#[derive(Default, Debug, Clone)]
struct VeryLongStateNameThatShouldStillWorkCorrectlyWithTheMacro {
    data: String,
}

impl AggregateState for VeryLongStateNameThatShouldStillWorkCorrectlyWithTheMacro {
    type Event = VeryLongEventNameThatShouldStillWorkCorrectlyWithTheMacro;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &VeryLongEventNameThatShouldStillWorkCorrectlyWithTheMacro) -> Self {
        match event {
            VeryLongEventNameThatShouldStillWorkCorrectlyWithTheMacro::SomethingHappenedWithAnExtremelyDescriptiveNameThatGoesOnAndOn(s) => {
                self.data = s.clone();
            }
        }
        self
    }
}

#[nexus::aggregate(
    state = VeryLongStateNameThatShouldStillWorkCorrectlyWithTheMacro,
    error = AError,
    id = AId
)]
struct VeryLongAggregateNameThatShouldStillWorkCorrectlyWithTheMacro;

#[test]
fn very_long_type_names() {
    let mut agg = VeryLongAggregateNameThatShouldStillWorkCorrectlyWithTheMacro::new(AId(1));
    agg.root_mut().apply_event(
        &VeryLongEventNameThatShouldStillWorkCorrectlyWithTheMacro::SomethingHappenedWithAnExtremelyDescriptiveNameThatGoesOnAndOn(
            "hello".into(),
        ),
    );
    assert_eq!(agg.state().data, "hello");
}

// =============================================================================
// Test: aggregate with path types (module::Type)
// =============================================================================

mod inner {
    use super::*;

    #[derive(Debug, Clone, DomainEvent)]
    pub enum InnerEvent {
        Ping,
    }

    #[derive(Default, Debug, Clone)]
    pub struct InnerState {
        pub pings: u32,
    }

    impl AggregateState for InnerState {
        type Event = InnerEvent;
        fn initial() -> Self {
            Self::default()
        }
        fn apply(mut self, _: &InnerEvent) -> Self {
            self.pings += 1;
            self
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("inner error")]
    pub struct InnerError;
}

#[nexus::aggregate(state = inner::InnerState, error = inner::InnerError, id = AId)]
struct PathTypeAggregate;

#[test]
fn aggregate_with_path_types() {
    let mut agg = PathTypeAggregate::new(AId(1));
    agg.root_mut().apply_event(&inner::InnerEvent::Ping);
    agg.root_mut().apply_event(&inner::InnerEvent::Ping);
    assert_eq!(agg.state().pings, 2);
}

// =============================================================================
// Test: event enum mixing all variant shapes
// =============================================================================

#[derive(Debug, Clone, DomainEvent)]
#[allow(
    dead_code,
    reason = "test-only event type — fields exist to test mixed variant shape generation"
)]
enum MixedShapeEvent {
    Unit,
    Tuple(String, u64),
    Struct { name: String, value: i32 },
    NestedTuple(Vec<String>),
}

#[test]
fn mixed_shape_event_names() {
    assert_eq!(MixedShapeEvent::Unit.name(), "Unit");
    assert_eq!(MixedShapeEvent::Tuple("a".into(), 1).name(), "Tuple");
    assert_eq!(
        MixedShapeEvent::Struct {
            name: "b".into(),
            value: 2,
        }
        .name(),
        "Struct"
    );
    assert_eq!(
        MixedShapeEvent::NestedTuple(vec!["c".into()]).name(),
        "NestedTuple"
    );
}

// =============================================================================
// Test: aggregate name that is a Rust keyword contextual identifier
// =============================================================================

#[nexus::aggregate(state = SingleState, error = AError, id = AId)]
struct r#Type; // `Type` is not a keyword but r# prefix works

#[test]
fn raw_identifier_aggregate_name() {
    let agg = r#Type::new(AId(1));
    assert_eq!(agg.version(), None);
}
