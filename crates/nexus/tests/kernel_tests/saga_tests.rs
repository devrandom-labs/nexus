//! Unit tests for the saga primitive (`React` trait + `AggregateRoot::react`).
//!
//! These tests cover the two cases the in-file `saga_dispatch_tests` module does
//! not: `correlate` returning `None` (defensive boundary), and `react` producing
//! more than one own-event (`N > 0`).

use nexus::{
    Aggregate, AggregateRoot, AggregateState, DomainEvent, Events, Id, Message, React, Saga,
    Version, events,
};

// ── Saga identity ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Hash, PartialEq, Eq, Default)]
struct OrderId([u8; 8]);

impl OrderId {
    const fn new(n: u64) -> Self {
        Self(n.to_le_bytes())
    }
}

impl core::fmt::Display for OrderId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", u64::from_le_bytes(self.0))
    }
}

impl AsRef<[u8]> for OrderId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Id for OrderId {
    const BYTE_LEN: usize = 8;
}

// ── The saga's OWN events (its history) ─────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum SagaEvent {
    PaymentRequested,
    OrderCompleted,
}

impl Message for SagaEvent {}

impl DomainEvent for SagaEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::PaymentRequested => "PaymentRequested",
            Self::OrderCompleted => "OrderCompleted",
        }
    }
}

// ── The saga's outgoing intent vocabulary ────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    TakePayment,
}

impl Message for Intent {}

// ── Saga state ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct OrderSagaState {
    payment_requested: bool,
    completed: bool,
}

impl AggregateState for OrderSagaState {
    type Event = SagaEvent;

    fn initial() -> Self {
        Self {
            payment_requested: false,
            completed: false,
        }
    }

    fn apply(mut self, event: &SagaEvent) -> Self {
        match event {
            SagaEvent::PaymentRequested => self.payment_requested = true,
            SagaEvent::OrderCompleted => self.completed = true,
        }
        self
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("order saga error")]
enum OrderSagaError {
    #[allow(
        dead_code,
        reason = "satisfies the Aggregate::Error bound; not raised by these tests"
    )]
    EmptyOrder,
}

// ── The saga marker ──────────────────────────────────────────────────────────

struct OrderSaga;

impl Aggregate for OrderSaga {
    type State = OrderSagaState;
    type Error = OrderSagaError;
    type Id = OrderId;
}

impl Saga for OrderSaga {
    type CorrelationKey = u64;
    type Command = Intent;

    fn intent_for(event: &SagaEvent) -> Option<Intent> {
        match event {
            SagaEvent::PaymentRequested => Some(Intent::TakePayment),
            SagaEvent::OrderCompleted => None,
        }
    }
}

// ── Upstream events ──────────────────────────────────────────────────────────

/// An upstream event this saga is never interested in routing.
#[derive(Debug)]
struct UnrelatedEvent;

impl Message for UnrelatedEvent {}

impl DomainEvent for UnrelatedEvent {
    fn name(&self) -> &'static str {
        "UnrelatedEvent"
    }
}

impl React<UnrelatedEvent> for OrderSaga {
    fn correlate(_event: &UnrelatedEvent) -> Option<u64> {
        None
    }

    fn react(
        _state: &OrderSagaState,
        _event: &UnrelatedEvent,
    ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
        Ok(None)
    }
}

/// Upstream event that drives two own-events at once (exercises `N > 0`).
#[derive(Debug)]
struct OrderExpedited {
    id: u64,
}

impl Message for OrderExpedited {}

impl DomainEvent for OrderExpedited {
    fn name(&self) -> &'static str {
        "OrderExpedited"
    }
}

impl React<OrderExpedited, 1> for OrderSaga {
    fn correlate(event: &OrderExpedited) -> Option<u64> {
        Some(event.id)
    }

    fn react(
        _state: &OrderSagaState,
        _event: &OrderExpedited,
    ) -> Result<Option<Events<SagaEvent, 1>>, OrderSagaError> {
        Ok(Some(events![
            SagaEvent::PaymentRequested,
            SagaEvent::OrderCompleted
        ]))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// `correlate` returning `None` means "not routed to any instance of this saga".
/// Defensive boundary: an upstream event type the saga does not handle must
/// produce `None`, not a spurious key.
#[test]
fn correlate_returns_none_for_unrouted_event() {
    assert_eq!(
        <OrderSaga as React<UnrelatedEvent>>::correlate(&UnrelatedEvent),
        None
    );
}

/// `react` can produce more than one own-event when `N > 0`.
/// Both events must survive the `AggregateRoot::react` dispatch path unchanged.
#[test]
fn react_produces_multiple_events_when_capacity_allows() {
    let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
    let produced = root
        .react::<OrderExpedited, 1>(&OrderExpedited { id: 1 })
        .expect("react must succeed")
        .expect("react must return Some events");
    assert_eq!(
        produced.into_iter().collect::<Vec<_>>(),
        vec![SagaEvent::PaymentRequested, SagaEvent::OrderCompleted]
    );
}

/// Smoke-test: the `react_ignores_when_no_op` path still works from the
/// external test surface (i.e., `Ok(None)` from `UnrelatedEvent::react`).
#[test]
fn react_returns_none_for_unrouted_event() {
    let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
    let outcome = root
        .react::<UnrelatedEvent, 0>(&UnrelatedEvent)
        .expect("react must not error");
    assert!(
        outcome.is_none(),
        "react for an unrouted event must yield None"
    );
}

/// Verify `Version` import is exercised: replaying a saga event advances the
/// root correctly before the multi-event react path.
#[test]
fn react_multi_event_after_replay() {
    let mut root = AggregateRoot::<OrderSaga>::new(OrderId::new(2));
    root.replay(Version::INITIAL, &SagaEvent::PaymentRequested)
        .expect("replay must succeed");
    // Even with prior history, OrderExpedited always produces two events.
    let produced = root
        .react::<OrderExpedited, 1>(&OrderExpedited { id: 2 })
        .expect("react must succeed")
        .expect("react must return Some events");
    assert_eq!(produced.into_iter().count(), 2);
}
