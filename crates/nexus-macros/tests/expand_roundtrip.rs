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
struct RtId([u8; 8]);
impl RtId {
    fn new(id: u64) -> Self {
        Self(id.to_be_bytes())
    }
}
impl fmt::Display for RtId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u64::from_be_bytes(self.0))
    }
}
impl AsRef<[u8]> for RtId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Id for RtId {
    const BYTE_LEN: usize = 8;
}

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
}

#[derive(Debug, thiserror::Error)]
#[error("rt error")]
struct RtError;

// --- What #[nexus::aggregate(...)] expands to ---
// (manually written, no macro)
//
// The macro now emits ONLY the unit marker struct plus `impl Aggregate`.
// No newtype field, no `new` constructor, no `AggregateEntity`, no `Debug`.
// Aggregates are constructed and driven via `AggregateRoot::<Name>::new(id)`.

struct RtAggregate;

impl ::nexus::Aggregate for RtAggregate {
    type State = RtState;
    type Error = RtError;
    type Id = RtId;
}

// --- Tests proving the expansion works identically to the macro ---

#[test]
fn expanded_lifecycle() {
    let mut root = AggregateRoot::<RtAggregate>::new(RtId::new(1));
    root.apply_event(&RtEvent::Added("one".into()));
    root.apply_event(&RtEvent::Added("two".into()));
    root.apply_event(&RtEvent::Removed);

    assert_eq!(root.state().items, vec!["one"]);
    // Version is None because apply_event does not advance version
    assert_eq!(root.version(), None);
}

#[test]
fn expanded_rehydrate() {
    let mut root = AggregateRoot::<RtAggregate>::new(RtId::new(1));
    root.replay(Version::INITIAL, &RtEvent::Added("a".into()))
        .unwrap();
    root.replay(
        Version::INITIAL.next().expect("version 2"),
        &RtEvent::Added("b".into()),
    )
    .unwrap();
    assert_eq!(root.state().items, vec!["a", "b"]);
    assert_eq!(root.version(), Version::new(2));
}

#[test]
fn expanded_root_accessors_work() {
    let mut root = AggregateRoot::<RtAggregate>::new(RtId::new(1));
    root.apply_event(&RtEvent::Added("test".into()));

    // AggregateRoot accessors
    assert_eq!(root.id(), &RtId::new(1));
    assert_eq!(root.state().items, vec!["test"]);
    // Version is None because apply_event does not advance version
    assert_eq!(root.version(), None);
}

#[test]
fn expanded_debug() {
    let root = AggregateRoot::<RtAggregate>::new(RtId::new(1));
    let debug = format!("{root:?}");
    assert!(debug.contains("AggregateRoot"));
}

#[test]
fn expanded_root_version_default() {
    let root = AggregateRoot::<RtAggregate>::new(RtId::new(1));
    assert_eq!(root.version(), None);
}
