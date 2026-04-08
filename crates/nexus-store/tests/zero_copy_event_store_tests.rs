//! Tests for `ZeroCopyEventStore` with `BorrowingCodec` (zero-copy path).

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(
    unsafe_code,
    reason = "test codec uses pointer cast to simulate zero-copy"
)]

use nexus::*;
use nexus_store::BorrowingCodec;
use nexus_store::event_store::ZeroCopyEventStore;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;
use std::fmt;

// -- Domain where Event is a fixed-layout type decodable from bytes --

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
struct CounterEvent {
    delta: i32,
}

impl Message for CounterEvent {}
impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        "CounterChanged"
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
struct CounterState {
    value: i64,
}
impl AggregateState for CounterState {
    type Event = CounterEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &CounterEvent) -> Self {
        self.value += i64::from(event.delta);
        self
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CounterId(u64);
impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ctr-{}", self.0)
    }
}
impl Id for CounterId {}

#[derive(Debug, thiserror::Error)]
#[error("counter error")]
struct CounterError;

struct CounterAggregate;
impl Aggregate for CounterAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = CounterId;
}

// -- Borrowing codec: reinterpret bytes as &CounterEvent --

struct CounterBorrowingCodec;

impl BorrowingCodec<CounterEvent> for CounterBorrowingCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        Ok(event.delta.to_le_bytes().to_vec())
    }

    fn decode<'a>(
        &self,
        _event_type: &str,
        payload: &'a [u8],
    ) -> Result<&'a CounterEvent, Self::Error> {
        if payload.len() != std::mem::size_of::<CounterEvent>() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "wrong payload size",
            ));
        }
        // SAFETY: CounterEvent is repr(C) with a single i32 field.
        // align_to handles alignment correctly.
        let (prefix, events, suffix) = unsafe { payload.align_to::<CounterEvent>() };
        if !prefix.is_empty() || suffix.is_empty() && events.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "alignment or size mismatch",
            ));
        }
        Ok(&events[0])
    }
}

#[tokio::test]
async fn zero_copy_save_and_load_roundtrip() {
    let es = ZeroCopyEventStore::new(InMemoryStore::new(), CounterBorrowingCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    let events = [CounterEvent { delta: 10 }, CounterEvent { delta: -3 }];
    es.save(&mut agg, &events).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 7);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn zero_copy_load_empty_stream() {
    let es = ZeroCopyEventStore::new(InMemoryStore::new(), CounterBorrowingCodec);
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 0);
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn zero_copy_multi_save_load() {
    let es = ZeroCopyEventStore::new(InMemoryStore::new(), CounterBorrowingCodec);

    let mut agg1 = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg1, &[CounterEvent { delta: 5 }])
        .await
        .unwrap();

    let mut agg2: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    es.save(&mut agg2, &[CounterEvent { delta: 3 }])
        .await
        .unwrap();

    let final_agg: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(final_agg.state().value, 8);
    assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
}
