//! Bounded saga repository (`SagaRepository`) integration tests — the 4
//! mandatory cross-cutting categories (CLAUDE.md rule 7) over `InMemoryStore`.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "panic in test match arms is an assertion")]
#![allow(
    clippy::shadow_reuse,
    reason = "the spawn-closure clone-and-shadow pattern is idiomatic for tokio tests"
)]
#![allow(
    clippy::missing_const_for_fn,
    reason = "test-helper fns need not be const"
)]

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::join_all;
use nexus::{
    Aggregate, AggregateRoot, AggregateState, DomainEvent, Events, Id, Message, React, Saga,
    Version,
};
use nexus_store::testing::InMemoryStore;
use nexus_store::{
    Decode, Encode, PersistedEnvelope, Reaction, Repository, SagaError, SagaRepository, Store,
};
use tokio::sync::Barrier;

// ── Saga identity (real definition) ──────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OrderId([u8; 8]);
impl OrderId {
    fn new(n: u64) -> Self {
        Self(n.to_le_bytes())
    }
    fn as_u64(&self) -> u64 {
        u64::from_le_bytes(self.0)
    }
}
impl core::fmt::Display for OrderId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_u64())
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

// ── The saga's OWN events (its history) ──────────────────────────────────
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

// ── Outgoing intent vocabulary (thin) ────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    TakePayment,
}
impl Message for Intent {}

// ── Saga state ───────────────────────────────────────────────────────────
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

// ── Upstream events the saga CONSUMES ────────────────────────────────────
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

// ── The saga marker ──────────────────────────────────────────────────────
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
            return Ok(None); // duplicate — already handled
        }
        Ok(Some(Events::new(SagaEvent::PaymentRequested)))
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
        Ok(Some(Events::new(SagaEvent::OrderCompleted)))
    }
}

// ── Codec for the saga's OWN events (no `json` feature in the gate) ───────
//
// `Decode::Error` must be `std::error::Error + Send + Sync + 'static`, so a
// bare `&'static str` does not satisfy the bound — use a thiserror enum.
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("bad saga event byte")]
struct BadSagaEventByte;

struct SagaCodec;
impl Encode<SagaEvent> for SagaCodec {
    type Error = Infallible;
    fn encode(&self, event: &SagaEvent) -> Result<Bytes, Self::Error> {
        let byte: u8 = match event {
            SagaEvent::PaymentRequested => 0,
            SagaEvent::OrderCompleted => 1,
        };
        Ok(Bytes::copy_from_slice(&[byte]))
    }
}
impl Decode<SagaEvent> for SagaCodec {
    type Output<'a> = SagaEvent;
    type Error = BadSagaEventByte;
    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<SagaEvent, Self::Error> {
        match env.payload().first() {
            Some(0) => Ok(SagaEvent::PaymentRequested),
            Some(1) => Ok(SagaEvent::OrderCompleted),
            _ => Err(BadSagaEventByte),
        }
    }
}

type Repo = nexus_store::EventStore<InMemoryStore, SagaCodec, OrderSaga>;

fn new_repo() -> Repo {
    Store::new(InMemoryStore::new())
        .repository()
        .codec(SagaCodec)
        .build()
}

// ── 1. Sequence/Protocol ─────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_then_reload_then_dispatch_advances_the_saga() {
    let repo = new_repo();
    let id = OrderId::new(1);

    // First upstream event → records PaymentRequested, projects TakePayment.
    let r1: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 1, total: 100 })
        .await
        .unwrap();
    match r1 {
        Reaction::Reacted { version, intents } => {
            assert_eq!(version, Version::INITIAL);
            let collected: Vec<_> = intents.iter().collect();
            assert_eq!(collected.len(), 1);
            assert_eq!(collected[0].intent(), &Intent::TakePayment);
            assert_eq!(collected[0].source_version(), Version::INITIAL);
            assert_eq!(collected[0].saga_id(), &id);
        }
        Reaction::Ignored => panic!("expected Reacted"),
    }

    // Second upstream event of a different type → records OrderCompleted,
    // which projects NO intent (internal-only).
    let r2: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &PaymentSettled { id: 1 })
        .await
        .unwrap();
    match r2 {
        Reaction::Reacted { version, intents } => {
            assert_eq!(version, Version::new(2).unwrap());
            assert!(intents.is_empty(), "OrderCompleted projects no intent");
        }
        Reaction::Ignored => panic!("expected Reacted"),
    }

    // Reload from the store: both own-events replayed → terminal state.
    let loaded: AggregateRoot<OrderSaga> = repo.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert!(loaded.state().completed);
    assert_eq!(loaded.version(), Version::new(2));
}

// ── 2. Lifecycle (write → reopen via a fresh facade over the same store) ──

#[tokio::test]
async fn reopen_facade_over_same_store_sees_prior_saga_events() {
    let backend = InMemoryStore::new();
    let store = Store::new(backend);
    let id = OrderId::new(7);

    {
        let repo: Repo = store.repository().codec(SagaCodec).build();
        let _: Reaction<OrderSaga, 0> = repo
            .dispatch(id.clone(), &OrderPlaced { id: 7, total: 50 })
            .await
            .unwrap();
    }
    // A brand-new facade over the SAME shared store handle.
    let repo2: Repo = store.repository().codec(SagaCodec).build();
    let loaded: AggregateRoot<OrderSaga> = repo2.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert_eq!(loaded.version(), Some(Version::INITIAL));
}

// ── 3. Defensive boundary ────────────────────────────────────────────────

#[tokio::test]
async fn react_ok_none_persists_nothing() {
    let repo = new_repo();
    let id = OrderId::new(2);

    // Prime the saga so payment_requested == true.
    let _: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 2, total: 10 })
        .await
        .unwrap();

    // A duplicate OrderPlaced now reacts to Ok(None) → Ignored, nothing written.
    let r: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 2, total: 10 })
        .await
        .unwrap();
    assert!(matches!(r, Reaction::Ignored));

    // Version unchanged from the single recorded event.
    let loaded: AggregateRoot<OrderSaga> = repo.load(id).await.unwrap();
    assert_eq!(loaded.version(), Some(Version::INITIAL));
}

#[tokio::test]
async fn react_error_rolls_back_nothing() {
    let repo = new_repo();
    let id = OrderId::new(3);

    let result: Result<Reaction<OrderSaga, 0>, _> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 3, total: 0 })
        .await;
    let err = result.unwrap_err();
    assert!(matches!(err, SagaError::React(OrderSagaError::EmptyOrder)));
    assert!(!err.is_conflict());

    // Nothing persisted: the stream is empty → fresh load at version None.
    let loaded: AggregateRoot<OrderSaga> = repo.load(id).await.unwrap();
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn stale_root_save_surfaces_conflict() {
    let repo = new_repo();
    let id = OrderId::new(4);

    // Load a fresh root (version None) and hold it.
    let mut stale: AggregateRoot<OrderSaga> = repo.load(id.clone()).await.unwrap();

    // A concurrent writer advances the stream to version 1.
    let _: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 4, total: 20 })
        .await
        .unwrap();

    // react_and_save against the STALE root expects version None → conflict.
    let err = repo
        .react_and_save::<OrderPlaced, 0>(&mut stale, &OrderPlaced { id: 4, total: 20 })
        .await
        .unwrap_err();
    assert!(
        err.is_conflict(),
        "stale expected-version must surface as conflict"
    );
    assert!(matches!(err, SagaError::Store(_)));
}

// ── 4. Linearizability/Isolation ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_dispatch_one_wins_loser_conflicts_then_retry_converges() {
    let repo = Arc::new(new_repo());
    let id = OrderId::new(5);

    // Two reactors race the SAME instance's first event. Both load version None,
    // both react, both try to append at expected version None → exactly one wins.
    let barrier = Arc::new(Barrier::new(2));
    let tasks = (0..2).map(|_| {
        let repo = Arc::clone(&repo);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            let mut root: AggregateRoot<OrderSaga> = repo.load(id.clone()).await.unwrap();
            barrier.wait().await; // maximize overlap on the append
            repo.react_and_save::<OrderPlaced, 0>(&mut root, &OrderPlaced { id: 5, total: 30 })
                .await
        })
    });
    let results: Vec<_> = join_all(tasks)
        .await
        .into_iter()
        .map(|j| j.unwrap())
        .collect();

    let wins = results.iter().filter(|r| r.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|r| r.as_ref().err().is_some_and(SagaError::is_conflict))
        .count();
    assert_eq!(wins, 1, "exactly one writer commits");
    assert_eq!(
        conflicts, 1,
        "the loser surfaces a conflict, not a silent drop"
    );

    // Caller-side retry (reload → re-dispatch) converges: the reload now sees
    // payment_requested == true, so the retry is a clean Ignored no-op.
    let retry: Reaction<OrderSaga, 0> = repo
        .dispatch(id.clone(), &OrderPlaced { id: 5, total: 30 })
        .await
        .unwrap();
    assert!(matches!(retry, Reaction::Ignored));

    // Final state: exactly one PaymentRequested recorded.
    let loaded: AggregateRoot<OrderSaga> = repo.load(id).await.unwrap();
    assert!(loaded.state().payment_requested);
    assert_eq!(loaded.version(), Some(Version::INITIAL));
}
