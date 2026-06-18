//! Tests for `nexus::testing::SagaFixture`. Exercises the real
//! given (replay) -> when (react) -> then chain on a sample saga, plus the
//! 4 mandatory cross-cutting categories (sequence, lifecycle, defensive
//! boundary; linearizability is N/A at the pure-primitive layer).
#![cfg(all(feature = "testing", feature = "derive"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    unused_must_use,
    reason = "test code; terminal then_expect_* assertions intentionally drop the returned fixture"
)]

use nexus::testing::SagaFixture;
use nexus::{AggregateState, DomainEvent, Events, Id, Message, React, Saga, events};

// ── Saga identity ────────────────────────────────────────────────
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
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

// ── The saga's OWN events (its history) ──────────────────────────
#[derive(Clone, Debug, PartialEq, Eq)]
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

// ── The saga's outgoing intents (thin commands) ──────────────────
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    TakePayment,
}
impl Message for Intent {}

// ── Saga state (folds its OWN events) ────────────────────────────
#[derive(Clone, Debug, Default, PartialEq)]
struct OrderSagaState {
    payment_requested: bool,
    completed: bool,
}
impl AggregateState for OrderSagaState {
    type Event = SagaEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &SagaEvent) -> Self {
        match event {
            SagaEvent::PaymentRequested => self.payment_requested = true,
            SagaEvent::OrderCompleted => self.completed = true,
        }
        self
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("order saga error")]
enum OrderSagaError {
    EmptyOrder,
}

// ── Upstream events the saga CONSUMES (transient input) ──────────
#[derive(Debug)]
struct OrderPlaced {
    id: u64,
    total: u64,
}
impl Message for OrderPlaced {}
impl DomainEvent for OrderPlaced {
    fn name(&self) -> &'static str {
        "OrderPlaced"
    }
}

#[derive(Debug)]
struct PaymentSettled {
    id: u64,
}
impl Message for PaymentSettled {}
impl DomainEvent for PaymentSettled {
    fn name(&self) -> &'static str {
        "PaymentSettled"
    }
}

// ── The saga marker (Aggregate half via macro; Saga + React by hand) ─
#[nexus::aggregate(state = OrderSagaState, error = OrderSagaError, id = OrderId)]
struct OrderSaga;

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

impl React<OrderPlaced> for OrderSaga {
    fn correlate(event: &OrderPlaced) -> Option<u64> {
        Some(event.id)
    }
    fn react(
        state: &OrderSagaState,
        event: &OrderPlaced,
    ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
        if event.total == 0 {
            return Err(OrderSagaError::EmptyOrder);
        }
        if state.payment_requested {
            return Ok(None);
        }
        Ok(Some(events![SagaEvent::PaymentRequested]))
    }
}

impl React<PaymentSettled> for OrderSaga {
    fn correlate(event: &PaymentSettled) -> Option<u64> {
        Some(event.id)
    }
    fn react(
        _state: &OrderSagaState,
        _event: &PaymentSettled,
    ) -> Result<Option<Events<SagaEvent>>, OrderSagaError> {
        Ok(Some(events![SagaEvent::OrderCompleted]))
    }
}

// Upstream event that drives TWO own-events at once (`N = 1`). Exercises the
// fixture's intent projection over multiple events: order-preserving, with the
// internal-only `OrderCompleted` filtered out of the intents.
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

// ── Basic given/when/then ────────────────────────────────────────
#[test]
fn react_on_empty_history_produces_event_and_intent() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(&OrderPlaced { id: 1, total: 100 })
        .then_expect_events([SagaEvent::PaymentRequested])
        .then_expect_commands([Intent::TakePayment])
        .then_expect_state(|s| assert!(s.payment_requested));
}

#[test]
fn with_id_constructs_equivalently_to_new() {
    SagaFixture::<OrderSaga>::with_id(OrderId::new(42))
        .given([])
        .when(&OrderPlaced { id: 42, total: 5 })
        .then_expect_events([SagaEvent::PaymentRequested]);
}

#[test]
fn internal_only_event_projects_no_intent() {
    // PaymentSettled -> OrderCompleted, which has no outgoing intent.
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested])
        .when(&PaymentSettled { id: 1 })
        .then_expect_events([SagaEvent::OrderCompleted])
        .then_expect_commands([])
        .then_expect_state(|s| assert!(s.completed));
}

#[test]
fn multi_event_react_projects_intents_in_order_filtering_internal() {
    // OrderExpedited (N = 1) produces two own-events; `intent_for` maps
    // PaymentRequested -> TakePayment and OrderCompleted -> None, so exactly one
    // intent is projected while BOTH events are folded, in order.
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(&OrderExpedited { id: 1 })
        .then_expect_events([SagaEvent::PaymentRequested, SagaEvent::OrderCompleted])
        .then_expect_commands([Intent::TakePayment])
        .then_expect_state(|s| assert!(s.payment_requested && s.completed));
}

// ── Category 1: Sequence/Protocol ────────────────────────────────
#[test]
fn sequence_react_after_prior_own_history_ignores_duplicate() {
    // The saga already requested payment (its own history); a duplicate
    // OrderPlaced must be ignored, leaving state unchanged.
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested])
        .when(&OrderPlaced { id: 1, total: 100 })
        .then_expect_ignored()
        .then_expect_state(|s| assert!(s.payment_requested && !s.completed));
}

// ── Category 2: Lifecycle (replay to a known state) ──────────────
#[test]
fn lifecycle_replay_full_history_reaches_terminal_state() {
    SagaFixture::<OrderSaga>::new()
        .given([SagaEvent::PaymentRequested, SagaEvent::OrderCompleted])
        .then_expect_state(|s| assert!(s.payment_requested && s.completed));
}

#[test]
#[should_panic(expected = "replaying saga history event")]
fn lifecycle_malformed_history_panics_in_given() {
    // `given` always feeds versions 1..=n in order, so the realistic replay
    // failure is the rehydration limit. A saga with MAX_REHYDRATION_EVENTS = 1
    // fed two events makes `replay` return Err, which `given` asserts on and
    // panics. See the `small_limit` module below.
    small_limit::trigger_panic();
}

mod small_limit {
    use super::{Intent, OrderId, OrderSagaError};
    use nexus::testing::SagaFixture;
    use nexus::{Aggregate, AggregateState, DomainEvent, Message, Saga};
    use std::num::NonZeroUsize;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Ev {
        Tick,
    }
    impl Message for Ev {}
    impl DomainEvent for Ev {
        fn name(&self) -> &'static str {
            "Tick"
        }
    }
    #[derive(Debug)]
    struct St;
    impl AggregateState for St {
        type Event = Ev;
        fn initial() -> Self {
            Self
        }
        fn apply(self, _: &Ev) -> Self {
            self
        }
    }
    struct Tiny;
    impl Aggregate for Tiny {
        type State = St;
        type Error = OrderSagaError;
        type Id = OrderId;
        const MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(1).unwrap();
    }
    impl Saga for Tiny {
        type CorrelationKey = u64;
        type Command = Intent;
        fn intent_for(_: &Ev) -> Option<Intent> {
            None
        }
    }

    pub fn trigger_panic() {
        // Replaying 2 events exceeds MAX_REHYDRATION_EVENTS = 1 → replay Err →
        // `given` asserts and panics with "replaying saga history event".
        let _ = SagaFixture::<Tiny>::new().given([Ev::Tick, Ev::Tick]);
    }
}

// ── Category 3: Defensive boundary ───────────────────────────────
#[test]
fn boundary_react_rejects_invalid_event_exact() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(&OrderPlaced { id: 1, total: 0 })
        .then_expect_error(OrderSagaError::EmptyOrder)
        .then_expect_state(|s| assert!(!s.payment_requested));
}

#[test]
fn boundary_react_rejects_invalid_event_matching() {
    SagaFixture::<OrderSaga>::new()
        .given([])
        .when(&OrderPlaced { id: 1, total: 0 })
        .then_expect_error_matching(|e| matches!(e, OrderSagaError::EmptyOrder));
}

// ── correlate (pure, tested directly — not via the fixture flow) ──
#[test]
fn correlate_extracts_key_per_upstream_event_type() {
    assert_eq!(
        <OrderSaga as React<OrderPlaced>>::correlate(&OrderPlaced { id: 7, total: 1 }),
        Some(7)
    );
    assert_eq!(
        <OrderSaga as React<PaymentSettled>>::correlate(&PaymentSettled { id: 9 }),
        Some(9)
    );
}
