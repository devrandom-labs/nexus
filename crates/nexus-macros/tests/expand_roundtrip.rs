//! Roundtrip expand-compile test.
//!
//! Verifies that the macro expansion is self-contained valid Rust.
//! We manually write what the macro SHOULD expand to and compile it
//! without any macros. If this compiles and passes, the expansion is correct.
//!
//! This catches cases where the macro generates code that only works
//! due to macro hygiene or implicit context.

use nexus::*;
use std::fmt;

// --- Domain types (same as used with macro) ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RtId(u64);
impl fmt::Display for RtId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for RtId {}

#[derive(Debug, Clone, PartialEq)]
enum RtEvent {
    Added(String),
    Removed,
}

// --- What #[derive(DomainEvent)] expands to ---
// (manually written, no macro)
impl ::nexus::Message for RtEvent {}
impl ::nexus::DomainEvent for RtEvent {
    fn name(&self) -> &'static str {
        match self {
            RtEvent::Added(..) => "Added",
            RtEvent::Removed => "Removed",
        }
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
struct RtState {
    items: Vec<String>,
}
impl AggregateState for RtState {
    type Event = RtEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &RtEvent) -> Self {
        match event {
            RtEvent::Added(s) => self.items.push(s.clone()),
            RtEvent::Removed => {
                self.items.pop();
            }
        }
        self
    }
    fn name(&self) -> &'static str {
        "Rt"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("rt error")]
struct RtError;

// --- What #[nexus::aggregate(...)] expands to ---
// (manually written, no macro)

struct RtAggregate(::nexus::AggregateRoot<RtAggregate>);

impl ::nexus::Aggregate for RtAggregate {
    type State = RtState;
    type Error = RtError;
    type Id = RtId;
}

impl ::nexus::AggregateEntity for RtAggregate {
    fn root(&self) -> &::nexus::AggregateRoot<Self> {
        &self.0
    }
    fn root_mut(&mut self) -> &mut ::nexus::AggregateRoot<Self> {
        &mut self.0
    }
}

impl RtAggregate {
    #[must_use]
    fn new(id: RtId) -> Self {
        Self(::nexus::AggregateRoot::new(id))
    }
}

impl ::std::fmt::Debug for RtAggregate {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("RtAggregate")
            .field("id", self.root().id())
            .field("version", &self.root().version())
            .finish_non_exhaustive()
    }
}

// --- Business logic (same as user would write) ---

impl RtAggregate {
    fn add(&mut self, item: String) {
        self.apply(RtEvent::Added(item));
    }

    fn remove(&mut self) {
        self.apply(RtEvent::Removed);
    }
}

// --- Tests proving the expansion works identically to the macro ---

#[test]
fn expanded_lifecycle() {
    let mut agg = RtAggregate::new(RtId(1));
    agg.add("one".into());
    agg.add("two".into());
    agg.remove();

    assert_eq!(agg.state().items, vec!["one"]);
    assert_eq!(agg.current_version(), Version::from_persisted(3));

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].event().name(), "Added");
    assert_eq!(events[2].event().name(), "Removed");
}

#[test]
fn expanded_rehydrate() {
    let mut agg = RtAggregate::new(RtId(1));
    agg.root_mut()
        .replay(Version::from_persisted(1), &RtEvent::Added("a".into()))
        .unwrap();
    agg.root_mut()
        .replay(Version::from_persisted(2), &RtEvent::Added("b".into()))
        .unwrap();
    assert_eq!(agg.state().items, vec!["a", "b"]);
    assert_eq!(agg.version(), Version::from_persisted(2));
}

#[test]
fn expanded_entity_trait_works() {
    let mut agg = RtAggregate::new(RtId(1));
    agg.add("test".into());

    // AggregateEntity methods
    assert_eq!(agg.id(), &RtId(1));
    assert_eq!(agg.state().items, vec!["test"]);
    assert_eq!(agg.version(), Version::INITIAL);
    assert_eq!(agg.current_version(), Version::from_persisted(1));
}

#[test]
fn expanded_debug() {
    let agg = RtAggregate::new(RtId(1));
    let debug = format!("{agg:?}");
    assert!(debug.contains("RtAggregate"));
}

#[test]
fn expanded_generic_entity_bound() {
    fn takes_entity<A: AggregateEntity>(agg: &A) -> Version {
        agg.version()
    }

    let agg = RtAggregate::new(RtId(1));
    assert_eq!(takes_entity(&agg), Version::INITIAL);
}
