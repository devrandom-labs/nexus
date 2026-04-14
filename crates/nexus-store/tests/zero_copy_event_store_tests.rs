//! Tests for `ZeroCopyEventStore` with `BorrowingCodec` (zero-copy path).

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(
    unsafe_code,
    reason = "test codec uses pointer cast to simulate zero-copy"
)]

use std::fmt;

use nexus::*;
use nexus_store::BorrowingCodec;
use nexus_store::Store;
use nexus_store::store::Repository;
use nexus_store::testing::InMemoryStore;

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
struct CounterId(String);
impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl AsRef<[u8]> for CounterId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for CounterId {
    const BYTE_LEN: usize = 0;
}

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
    let store = Store::new(InMemoryStore::new());
    let es = store
        .repository()
        .codec(CounterBorrowingCodec)
        .build_zero_copy();

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId("ctr-1".into()));
    let events = [CounterEvent { delta: 10 }, CounterEvent { delta: -3 }];
    es.save(&mut agg, &events).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(loaded.state().value, 7);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn zero_copy_load_empty_stream() {
    let store = Store::new(InMemoryStore::new());
    let es = store
        .repository()
        .codec(CounterBorrowingCodec)
        .build_zero_copy();
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(loaded.state().value, 0);
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn zero_copy_multi_save_load() {
    let store = Store::new(InMemoryStore::new());
    let es = store
        .repository()
        .codec(CounterBorrowingCodec)
        .build_zero_copy();

    let mut agg1 = AggregateRoot::<CounterAggregate>::new(CounterId("ctr-1".into()));
    es.save(&mut agg1, &[CounterEvent { delta: 5 }])
        .await
        .unwrap();

    let mut agg2: AggregateRoot<CounterAggregate> =
        es.load(CounterId("ctr-1".into())).await.unwrap();
    es.save(&mut agg2, &[CounterEvent { delta: 3 }])
        .await
        .unwrap();

    let final_agg: AggregateRoot<CounterAggregate> =
        es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(final_agg.state().value, 8);
    assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
}
