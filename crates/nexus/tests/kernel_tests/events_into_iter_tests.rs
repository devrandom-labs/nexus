//! Tests that the owning iterator of `Events` is the named, leak-free
//! `EventsIntoIter` type and preserves Chain's iteration capabilities.

use nexus::{DomainEvent, Events, EventsIntoIter, events};

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestEvent {
    A,
    B,
    C,
}

impl nexus::Message for TestEvent {}

impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }
}

#[test]
fn into_iter_returns_the_named_sealed_type() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    // The associated `IntoIter` is `EventsIntoIter`, not an arrayvec type.
    let it: EventsIntoIter<TestEvent, 2> = evts.into_iter();
    let collected: Vec<TestEvent> = it.collect();
    assert_eq!(collected, vec![TestEvent::A, TestEvent::B, TestEvent::C]);
}

#[test]
fn yields_first_then_rest_in_order() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B];
    let collected: Vec<TestEvent> = evts.into_iter().collect();
    assert_eq!(collected, vec![TestEvent::A, TestEvent::B]);
}

#[test]
fn is_double_ended() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    let reversed: Vec<TestEvent> = evts.into_iter().rev().collect();
    assert_eq!(reversed, vec![TestEvent::C, TestEvent::B, TestEvent::A]);
}

#[test]
fn is_fused_after_exhaustion() {
    let evts: Events<TestEvent, 0> = events![TestEvent::A];
    let mut it = evts.into_iter();
    assert_eq!(it.next(), Some(TestEvent::A));
    assert_eq!(it.next(), None);
    assert_eq!(it.next(), None);
}

#[test]
fn size_hint_counts_all_events() {
    let evts: Events<TestEvent, 2> = events![TestEvent::A, TestEvent::B, TestEvent::C];
    let it = evts.into_iter();
    assert_eq!(it.size_hint(), (3, Some(3)));
}
