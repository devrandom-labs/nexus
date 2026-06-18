//! Saga / process-manager primitive — the dual of an aggregate.
//!
//! Where an aggregate consumes a **command** and produces its own **events**
//! ([`Handle`](crate::Handle)), a saga consumes an upstream **event** and
//! produces its own **events** ([`React`]). The command an aggregate consumes is
//! never folded into its state — it is transient input; only the produced events
//! fold. The upstream event plays the same role for a saga: it is the transient
//! input to [`React::react`], and the saga folds only its *own* events (one
//! stream), exactly like an aggregate. A saga therefore reuses all aggregate
//! machinery ([`Aggregate`], [`AggregateState`](crate::AggregateState),
//! [`AggregateRoot`], [`replay`](AggregateRoot::replay), version tracking) and
//! adds only three things:
//!
//! 1. [`React::correlate`] — which running instance an incoming event belongs to
//!    (pure key *extraction*; instance *resolution* is the runtime's concern).
//! 2. [`React`] instead of [`Handle`](crate::Handle) — one impl per upstream
//!    event type the saga consumes.
//! 3. [`Saga::intent_for`] — derive at most one outgoing intent from each of the
//!    saga's own events (Model A: commands are a 1:1 projection of events, never
//!    a second output, so a dispatch can never drift from a recorded event).
//!
//! See `docs/plans/2026-06-18-saga-primitive-design.md`.

use crate::aggregate::{Aggregate, AggregateRoot, EventOf};
use crate::event::DomainEvent;
use crate::events::Events;
use crate::message::Message;
use core::fmt::Debug;
use core::hash::Hash;

/// Saga-level contract. A saga **is** an aggregate whose events are its own
/// history, plus how to route incoming events and how each own-event maps to an
/// outgoing intent.
///
/// Implement [`React`] (the dual of [`Handle`](crate::Handle)) for each upstream
/// event type the saga consumes. Because `Saga: Aggregate`, obtain the
/// [`Aggregate`] half from `#[nexus::aggregate(..)]` and hand-write `impl Saga`
/// + `impl React`.
pub trait Saga: Aggregate {
    /// Which running instance an incoming event belongs to.
    ///
    /// The runtime (Agency) resolves key → instance; nexus only extracts the
    /// key (see [`React::correlate`]). Bounded so the runtime can use it as a
    /// map key.
    type CorrelationKey: Clone + Eq + Hash + Send + Sync + Debug + 'static;

    /// The saga's emitted intent vocabulary — *thin* requests, enriched into
    /// fat commands by the runtime before reaching a target aggregate. Not tied
    /// to any target aggregate's command type.
    type Command: Message;

    /// Derive the outgoing intent implied by one of the saga's own events.
    ///
    /// At most one intent per event (`None` for internal-only events such as a
    /// terminal `Completed`). Must be a dumb, total lookup with no branching —
    /// all branching already happened in [`React::react`] when it chose *which*
    /// event to record.
    fn intent_for(event: &EventOf<Self>) -> Option<Self::Command>;
}

/// Per-incoming-event-type handler — the **react** function, dual of
/// [`Handle<C, N>`](crate::Handle).
///
/// The const generic `N` controls how many *additional* own-events beyond the
/// first can be produced; total capacity is `N + 1` (default `N = 0` = exactly
/// one own-event). Implement one `React` per upstream event type the saga
/// consumes (the dual of one `Handle` per command type).
pub trait React<E: DomainEvent, const N: usize = 0>: Saga {
    /// Pure correlation-key extraction for this incoming event type.
    ///
    /// `None` means the event is not routed to any instance of this saga.
    fn correlate(event: &E) -> Option<Self::CorrelationKey>;

    /// React to an incoming upstream event, given the current saga state.
    ///
    /// - `Ok(None)` — not interested / no-op (records nothing); e.g. a duplicate
    ///   or a step already passed.
    /// - `Ok(Some(events))` — the saga's own events (at least one), which advance
    ///   state via [`AggregateState::apply`](crate::AggregateState::apply).
    ///
    /// Takes `&E` (borrow) so events read from a stream are not cloned, mirroring
    /// [`AggregateRoot::replay`]. (`handle` takes the command by value because
    /// the caller constructs it fresh; upstream events are read from elsewhere.)
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the incoming event violates a saga invariant.
    fn react(
        state: &Self::State,
        event: &E,
    ) -> Result<Option<Events<EventOf<Self>, N>>, Self::Error>;
}

impl<A: Aggregate> AggregateRoot<A> {
    /// React to an incoming upstream event against the current state.
    ///
    /// Dispatches to the saga's [`React`] impl, passing the current
    /// [`state`](Self::state). Pure — reads state, returns the produced
    /// own-events (or `None`), mutates nothing. The dual of
    /// [`handle`](Self::handle).
    ///
    /// # Errors
    ///
    /// Returns `A::Error` when the incoming event violates a saga invariant.
    pub fn react<E, const N: usize>(
        &self,
        event: &E,
    ) -> Result<Option<Events<EventOf<A>, N>>, A::Error>
    where
        E: DomainEvent,
        A: React<E, N>,
    {
        A::react(self.state(), event)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code")]
mod saga_dispatch_tests {
    use super::{React, Saga};
    use crate::aggregate::{Aggregate, AggregateRoot, AggregateState};
    use crate::event::DomainEvent;
    use crate::events;
    use crate::events::Events;
    use crate::id::Id;
    use crate::message::Message;
    use crate::version::Version;

    // ── Saga identity ────────────────────────────────────────────────
    #[derive(Debug, Clone, Hash, PartialEq, Eq, Default)]
    struct OrderId([u8; 8]);
    impl OrderId {
        fn new(n: u64) -> Self {
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

    // ── The saga's outgoing intent vocabulary (thin commands) ────────
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Intent {
        TakePayment,
    }
    impl Message for Intent {}

    // ── Saga state (folds its OWN events) ────────────────────────────
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

    // ── The saga marker ──────────────────────────────────────────────
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
                SagaEvent::OrderCompleted => None, // internal-only
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
                return Ok(None); // already handled — ignore duplicate
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

    #[test]
    fn react_produces_own_events_via_dispatch() {
        let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        let produced = root
            .react(&OrderPlaced { id: 1, total: 100 })
            .expect("ok")
            .expect("some");
        assert_eq!(
            produced.into_iter().collect::<Vec<_>>(),
            vec![SagaEvent::PaymentRequested]
        );
    }

    #[test]
    fn react_ignores_when_no_op() {
        // Replay a history where payment was already requested, then a second
        // OrderPlaced is a duplicate → Ok(None).
        let mut root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        root.replay(Version::INITIAL, &SagaEvent::PaymentRequested)
            .expect("replay");
        let outcome = root.react(&OrderPlaced { id: 1, total: 100 }).expect("ok");
        assert!(outcome.is_none());
    }

    #[test]
    fn react_surfaces_saga_error() {
        let root = AggregateRoot::<OrderSaga>::new(OrderId::new(1));
        assert_eq!(
            root.react::<OrderPlaced, 0>(&OrderPlaced { id: 1, total: 0 }),
            Err(OrderSagaError::EmptyOrder)
        );
    }

    #[test]
    fn correlate_extracts_key_per_event_type() {
        assert_eq!(
            <OrderSaga as React<OrderPlaced>>::correlate(&OrderPlaced { id: 42, total: 9 }),
            Some(42)
        );
        assert_eq!(
            <OrderSaga as React<PaymentSettled>>::correlate(&PaymentSettled { id: 7 }),
            Some(7)
        );
    }

    #[test]
    fn intent_for_projects_events_one_to_one() {
        assert_eq!(
            OrderSaga::intent_for(&SagaEvent::PaymentRequested),
            Some(Intent::TakePayment)
        );
        assert_eq!(OrderSaga::intent_for(&SagaEvent::OrderCompleted), None);
    }
}
