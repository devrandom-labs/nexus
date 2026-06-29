//! Tests for a borrowing (zero-copy) codec — both the [`Decode`] trait's
//! borrowing GAT shape directly, and end-to-end through the unified
//! [`EventStore`](nexus_store::EventStore) facade via `.build()`.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(
    unsafe_code,
    reason = "test codec uses pointer cast to simulate zero-copy"
)]

use std::fmt;

use nexus::*;
use nexus_store::Repository;
use nexus_store::Store;
use nexus_store::testing::InMemoryStore;
use nexus_store::{Decode, Encode, PersistedEnvelope};

/// A test codec that "decodes" by reinterpreting bytes as a u32 slice.
struct U32Codec;

impl Encode<[u32]> for U32Codec {
    type Error = std::io::Error;

    fn encode(&self, event: &[u32]) -> Result<bytes::Bytes, Self::Error> {
        let buf: Vec<u8> = event.iter().flat_map(|n| n.to_le_bytes()).collect();
        Ok(bytes::Bytes::from(buf))
    }
}

impl Decode<[u32]> for U32Codec {
    type Output<'a> = &'a [u32];
    type Error = std::io::Error;

    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<&'a [u32], Self::Error> {
        let payload = env.payload();
        if !payload.len().is_multiple_of(4) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "payload length not multiple of 4",
            ));
        }
        let (prefix, shorts, suffix) = unsafe { payload.align_to::<u32>() };
        if !prefix.is_empty() || !suffix.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unaligned",
            ));
        }
        Ok(shorts)
    }
}

#[test]
fn borrowing_codec_decode_borrows_from_payload() {
    let codec = U32Codec;
    let original: Vec<u32> = vec![1, 2, 3];
    let encoded = codec.encode(&original).unwrap();
    let env = PersistedEnvelope::for_decode("event", &encoded).unwrap();
    let decoded = codec.decode(&env).unwrap();
    assert_eq!(decoded, &[1, 2, 3]);
}

#[test]
fn borrowing_codec_decode_rejects_bad_payload() {
    let codec = U32Codec;
    let bad = vec![1, 2, 3];
    let env = PersistedEnvelope::for_decode("event", &bad).unwrap();
    assert!(codec.decode(&env).is_err());
}

#[test]
fn borrowing_codec_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<U32Codec>();
}

// ─── Facade-level tests: borrowing codec through the unified EventStore ──────

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

impl Encode<CounterEvent> for CounterBorrowingCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<bytes::Bytes, Self::Error> {
        Ok(bytes::Bytes::copy_from_slice(&event.delta.to_le_bytes()))
    }
}

impl Decode<CounterEvent> for CounterBorrowingCodec {
    type Output<'a> = &'a CounterEvent;
    type Error = std::io::Error;

    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<&'a CounterEvent, Self::Error> {
        let payload = env.payload();
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

// PR1 bytes-envelope refactor regression: the new `PersistedEnvelope` exposes
// `payload()` as a slice into a shared `Bytes` buffer at offset
// `event_type.len()`. That offset is rarely aligned to `align_of::<T>()` for
// arbitrary `T`, so zero-copy borrowing decoders that require alignment
// (`payload.align_to::<T>()`) cannot return a borrowed `&T` here.
//
// Restoring the implicit alignment guarantee (which the old `payload: Vec<u8>`
// satisfied incidentally via the allocator) requires wire-format work and is
// scoped to a follow-up. See deviation log
// `2026-05-27-bytes-envelope-deviations.md`.
#[tokio::test]
async fn zero_copy_save_and_load_roundtrip() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(CounterBorrowingCodec).build();

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId("ctr-1".into()));
    let events = [CounterEvent { delta: 10 }, CounterEvent { delta: -3 }];
    es.save(&mut agg, &events_from_slice(&events))
        .await
        .unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(loaded.state().value, 7);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn zero_copy_load_empty_stream() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(CounterBorrowingCodec).build();
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(loaded.state().value, 0);
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn zero_copy_multi_save_load() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(CounterBorrowingCodec).build();

    let mut agg1 = AggregateRoot::<CounterAggregate>::new(CounterId("ctr-1".into()));
    es.save(&mut agg1, &events_from_slice(&[CounterEvent { delta: 5 }]))
        .await
        .unwrap();

    let mut agg2: AggregateRoot<CounterAggregate> =
        es.load(CounterId("ctr-1".into())).await.unwrap();
    es.save(&mut agg2, &events_from_slice(&[CounterEvent { delta: 3 }]))
        .await
        .unwrap();

    let final_agg: AggregateRoot<CounterAggregate> =
        es.load(CounterId("ctr-1".into())).await.unwrap();
    assert_eq!(final_agg.state().value, 8);
    assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
}

// ─── #207 test helper ──────────────────────────────────────────────────────
// `Repository::save` takes `&Events<E, N>` (non-empty, compile-time capacity).
// These tests build batches from runtime-length slices/proptest vectors, so we
// pack them into `Events<E, 32>` (capacity 33 — covers every batch built here;
// the largest strategy yields 29). Empty input is a programmer error: `save`
// makes a zero-event batch unrepresentable by construction.
fn events_from_slice<E: nexus::DomainEvent + Clone>(slice: &[E]) -> nexus::Events<E, 32> {
    let (first, rest) = slice
        .split_first()
        .expect("save requires at least one event");
    let mut events = nexus::Events::new(first.clone());
    for event in rest {
        events.add(event.clone());
    }
    events
}
