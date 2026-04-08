//! Principal QA test suite — 11 dimensions of adversarial testing for nexus-store.
//!
//! Each dimension targets a specific class of bugs through the Repository/EventStore
//! layer. The goal is to break the crate and expose correctness issues.
//!
//! # Dimensions
//!
//! 1. **Codec Failure Injection** — encode/decode failures at various points
//! 2. **Save Atomicity** — data loss when save fails mid-operation
//! 3. **Concurrent Access** — race conditions and optimistic concurrency
//! 4. **Upcaster Chain Integration** — schema evolution through EventStore
//! 5. **Version Boundary Arithmetic** — edge cases in version tracking
//! 6. **Aggregate Lifecycle State Machine** — multi-round persistence cycles
//! 7. **Repository Contract Verification** — trait contract enforcement
//! 8. **Error Propagation Paths** — every error variant through the stack
//! 9. **Property-Based Algebraic Laws** — event sourcing invariants via proptest
//! 10. **Resource Limits** — MAX_REHYDRATION_EVENTS, MAX_UNCOMMITTED, large batches
//! 11. **InMemoryStore Edge Cases** — adapter-specific bugs and limitations

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_docs_in_private_items, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "test readability")]
#![allow(clippy::as_conversions, reason = "test assertions")]
#![allow(clippy::arc_with_non_send_sync, reason = "test fixtures")]
#![allow(clippy::clone_on_ref_ptr, reason = "Arc::clone in tests")]
#![allow(clippy::doc_markdown, reason = "test comments")]
#![allow(clippy::missing_const_for_fn, reason = "test readability")]
#![allow(clippy::panic, reason = "tests use panic for assertions")]
#![allow(clippy::cast_lossless, reason = "test simplicity")]
#![allow(clippy::shadow_reuse, reason = "Arc::clone shadowing in spawned tasks")]
#![allow(clippy::manual_let_else, reason = "test readability")]
#![allow(
    unsafe_code,
    reason = "test codec uses pointer cast to simulate zero-copy"
)]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use nexus::*;
use nexus_store::BorrowingCodec;
use nexus_store::codec::Codec;
use nexus_store::error::StoreError;
use nexus_store::event_store::{EventStore, ZeroCopyEventStore};
use nexus_store::morsel::EventMorsel;
use nexus_store::raw::RawEventStore;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;
use nexus_store::{UpcastError, Upcaster};
use proptest::prelude::*;

// StreamId has been removed from the API — use typed Id values directly

// ═══════════════════════════════════════════════════════════════════════════
// Test domain: Counter aggregate (owning codec path)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum CounterEvent {
    Incremented,
    Decremented,
    Set(i64),
}

impl Message for CounterEvent {}

impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Incremented => "Incremented",
            Self::Decremented => "Decremented",
            Self::Set(_) => "Set",
        }
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
        match event {
            CounterEvent::Incremented => self.value += 1,
            CounterEvent::Decremented => self.value -= 1,
            CounterEvent::Set(v) => self.value = *v,
        }
        self
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CounterId(u64);

impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "counter-{}", self.0)
    }
}

impl Id for CounterId {}

#[derive(Debug, thiserror::Error)]
#[error("counter error")]
struct CounterError;

#[derive(Debug)]
struct CounterAggregate;

impl Aggregate for CounterAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = CounterId;
}

/// Aggregate with very low limits — for testing resource exhaustion.
#[derive(Debug)]
struct TinyAggregate;

impl Aggregate for TinyAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = CounterId;
    const MAX_REHYDRATION_EVENTS: std::num::NonZeroUsize = {
        // SAFETY: 5 is non-zero
        match std::num::NonZeroUsize::new(5) {
            Some(v) => v,
            None => unreachable!(),
        }
    };
}

// ═══════════════════════════════════════════════════════════════════════════
// Test domain: Delta aggregate (borrowing codec / zero-copy path)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
struct DeltaEvent {
    delta: i32,
}

impl Message for DeltaEvent {}

impl DomainEvent for DeltaEvent {
    fn name(&self) -> &'static str {
        "Delta"
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
struct DeltaState {
    total: i64,
}

impl AggregateState for DeltaState {
    type Event = DeltaEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &DeltaEvent) -> Self {
        self.total += i64::from(event.delta);
        self
    }
}

#[derive(Debug)]
struct DeltaAggregate;

impl Aggregate for DeltaAggregate {
    type State = DeltaState;
    type Error = CounterError;
    type Id = CounterId;
}

// ═══════════════════════════════════════════════════════════════════════════
// Test codecs
// ═══════════════════════════════════════════════════════════════════════════

/// Correct owning codec for `CounterEvent`.
struct SimpleCodec;

impl Codec<CounterEvent> for SimpleCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        match event {
            CounterEvent::Incremented => Ok(b"inc".to_vec()),
            CounterEvent::Decremented => Ok(b"dec".to_vec()),
            CounterEvent::Set(v) => Ok(format!("set:{v}").into_bytes()),
        }
    }

    fn decode(&self, _event_type: &str, payload: &[u8]) -> Result<CounterEvent, Self::Error> {
        let s = std::str::from_utf8(payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if s == "inc" {
            Ok(CounterEvent::Incremented)
        } else if s == "dec" {
            Ok(CounterEvent::Decremented)
        } else if let Some(v) = s.strip_prefix("set:") {
            let n: i64 = v
                .parse()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(CounterEvent::Set(n))
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown event payload: {s}"),
            ))
        }
    }
}

/// Codec that fails after N successful encodes.
struct FailAfterNEncodesCodec {
    max_encodes: usize,
    encode_count: AtomicUsize,
}

impl FailAfterNEncodesCodec {
    fn new(max_encodes: usize) -> Self {
        Self {
            max_encodes,
            encode_count: AtomicUsize::new(0),
        }
    }
}

impl Codec<CounterEvent> for FailAfterNEncodesCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        let count = self.encode_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_encodes {
            return Err(std::io::Error::other(format!(
                "encode limit reached (call #{count})"
            )));
        }
        SimpleCodec.encode(event)
    }

    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<CounterEvent, Self::Error> {
        SimpleCodec.decode(event_type, payload)
    }
}

/// Codec that fails after N successful decodes.
struct FailAfterNDecodesCodec {
    inner: SimpleCodec,
    max_decodes: usize,
    decode_count: AtomicUsize,
}

impl FailAfterNDecodesCodec {
    fn new(max_decodes: usize) -> Self {
        Self {
            inner: SimpleCodec,
            max_decodes,
            decode_count: AtomicUsize::new(0),
        }
    }
}

impl Codec<CounterEvent> for FailAfterNDecodesCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        self.inner.encode(event)
    }

    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<CounterEvent, Self::Error> {
        let count = self.decode_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_decodes {
            return Err(std::io::Error::other(format!(
                "decode limit reached (call #{count})"
            )));
        }
        self.inner.decode(event_type, payload)
    }
}

/// Codec with a runtime toggle — shared `Arc<AtomicBool>` lets the test
/// flip between working and broken states after EventStore takes ownership.
struct ToggleableCodec {
    fail_flag: Arc<AtomicBool>,
}

impl Codec<CounterEvent> for ToggleableCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        if self.fail_flag.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("toggled to fail"));
        }
        SimpleCodec.encode(event)
    }

    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<CounterEvent, Self::Error> {
        if self.fail_flag.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("toggled to fail"));
        }
        SimpleCodec.decode(event_type, payload)
    }
}

/// Zero-copy codec for `DeltaEvent` — reinterprets bytes as `&DeltaEvent`.
struct DeltaBorrowingCodec;

impl BorrowingCodec<DeltaEvent> for DeltaBorrowingCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &DeltaEvent) -> Result<Vec<u8>, Self::Error> {
        Ok(event.delta.to_le_bytes().to_vec())
    }

    fn decode<'a>(
        &self,
        _event_type: &str,
        payload: &'a [u8],
    ) -> Result<&'a DeltaEvent, Self::Error> {
        if payload.len() != std::mem::size_of::<DeltaEvent>() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "wrong payload size for DeltaEvent",
            ));
        }
        // SAFETY: DeltaEvent is repr(C) with a single i32 field.
        let (prefix, events, _suffix) = unsafe { payload.align_to::<DeltaEvent>() };
        if !prefix.is_empty() || events.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "alignment mismatch for DeltaEvent",
            ));
        }
        Ok(&events[0])
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test upcasters
// ═══════════════════════════════════════════════════════════════════════════

/// Upcaster: bumps "Incremented" from schema v1→v2 (payload unchanged).
struct IncrementedV1ToV2;

impl Upcaster for IncrementedV1ToV2 {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        match (morsel.event_type(), morsel.schema_version()) {
            ("Incremented", v) if v == Version::INITIAL => Ok(EventMorsel::new(
                "Incremented",
                Version::new(2).unwrap(),
                morsel.payload().to_vec(),
            )),
            _ => Ok(morsel),
        }
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "Incremented" => Some(Version::new(2).unwrap()),
            _ => None,
        }
    }
}

/// Upcaster: bumps "Incremented" from v1→v3 in two steps (v1→v2→v3).
struct IncrementedV1ToV3;

impl Upcaster for IncrementedV1ToV3 {
    fn apply<'a>(&self, mut morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        loop {
            morsel = match (morsel.event_type(), morsel.schema_version()) {
                ("Incremented", v) if v == Version::INITIAL => EventMorsel::new(
                    "Incremented",
                    Version::new(2).unwrap(),
                    morsel.payload().to_vec(),
                ),
                ("Incremented", v) if v == Version::new(2).unwrap() => EventMorsel::new(
                    "Incremented",
                    Version::new(3).unwrap(),
                    morsel.payload().to_vec(),
                ),
                _ => break,
            };
        }
        Ok(morsel)
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "Incremented" => Some(Version::new(3).unwrap()),
            _ => None,
        }
    }
}

/// Upcaster that doubles the i32 delta — actually mutates payload bytes.
struct DeltaDoublingUpcaster;

impl Upcaster for DeltaDoublingUpcaster {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        match (morsel.event_type(), morsel.schema_version()) {
            ("Delta", v) if v == Version::INITIAL => {
                let delta = i32::from_le_bytes(morsel.payload()[0..4].try_into().unwrap());
                let doubled = delta * 2;
                Ok(EventMorsel::new(
                    "Delta",
                    Version::new(2).unwrap(),
                    doubled.to_le_bytes().to_vec(),
                ))
            }
            _ => Ok(morsel),
        }
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "Delta" => Some(Version::new(2).unwrap()),
            _ => None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 1: Codec Failure Injection
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d1_encode_failure_mid_batch_returns_codec_error() {
    // Encode succeeds for 1st event, fails for 2nd
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(1));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    let result = es
        .save(
            &mut agg,
            &[
                CounterEvent::Incremented,
                CounterEvent::Incremented,
                CounterEvent::Incremented,
            ],
        )
        .await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), StoreError::Codec(_)),
        "mid-batch encode failure must surface as StoreError::Codec"
    );
}

#[tokio::test]
async fn d1_encode_failure_first_event_returns_codec_error() {
    // All encodes fail — no events should reach the store
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    let result = es.save(&mut agg, &[CounterEvent::Incremented]).await;
    assert!(matches!(result.unwrap_err(), StoreError::Codec(_)));
}

#[tokio::test]
async fn d1_decode_failure_mid_stream_returns_codec_error() {
    // FailAfterNDecodesCodec: encode always succeeds, decode fails after N calls.
    // Save 3 events (encodes fine), then load (2nd decode fails).
    let es = EventStore::new(InMemoryStore::new(), FailAfterNDecodesCodec::new(1));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Load — first decode succeeds, second fails
    let result: Result<AggregateRoot<CounterAggregate>, _> = es.load(CounterId(1)).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), StoreError::Codec(_)),
        "mid-stream decode failure must surface as StoreError::Codec"
    );
}

#[tokio::test]
async fn d1_decode_failure_first_event_returns_codec_error() {
    // Decode fails on the very first event
    let es = EventStore::new(InMemoryStore::new(), FailAfterNDecodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap(); // encode works fine

    let result: Result<AggregateRoot<CounterAggregate>, _> = es.load(CounterId(1)).await;
    assert!(matches!(result.unwrap_err(), StoreError::Codec(_)));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 2: Save Atomicity — version must not advance on failed save
//
// In the new API, events are passed to save() directly. The aggregate's
// version must not advance if encoding or appending fails.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d2_failed_encode_must_not_advance_version() {
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    assert_eq!(agg.version(), None);

    // Save fails (encode error)
    let result = es
        .save(
            &mut agg,
            &[CounterEvent::Incremented, CounterEvent::Incremented],
        )
        .await;
    assert!(result.is_err());

    // CORRECT: version must NOT advance on failed save
    assert_eq!(
        agg.version(),
        None,
        "version must remain None after failed save — no events were persisted"
    );
}

#[tokio::test]
async fn d2_aggregate_must_be_retryable_after_failed_save() {
    let fail_flag = Arc::new(AtomicBool::new(true));
    let codec = ToggleableCodec {
        fail_flag: fail_flag.clone(),
    };
    let es = EventStore::new(InMemoryStore::new(), codec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    let events = [CounterEvent::Incremented, CounterEvent::Incremented];

    // First save fails
    let result = es.save(&mut agg, &events).await;
    assert!(result.is_err());

    // Fix the codec
    fail_flag.store(false, Ordering::SeqCst);

    // CORRECT: retry must succeed — same events should persist
    let retry = es.save(&mut agg, &events).await;
    assert!(
        retry.is_ok(),
        "aggregate must be retryable after a failed save — got: {:?}",
        retry.unwrap_err()
    );

    // CORRECT: loading must return the persisted state
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 2);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 3: Concurrent Access
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d3_concurrent_saves_one_wins_one_conflicts() {
    let es = Arc::new(EventStore::new(InMemoryStore::new(), SimpleCodec));

    // Setup: one event in the stream
    let mut setup = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut setup, &[CounterEvent::Set(0)]).await.unwrap();

    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    // Task A: load, wait for B, save
    let handle_a = tokio::spawn({
        let es = es.clone();
        let barrier = barrier.clone();
        async move {
            let mut agg: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
            barrier.wait().await; // both loaded before either saves
            es.save(&mut agg, &[CounterEvent::Set(100)]).await
        }
    });

    // Task B: load, wait for A, save
    let handle_b = tokio::spawn({
        let es = es.clone();
        let barrier = barrier.clone();
        async move {
            let mut agg: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
            barrier.wait().await; // both loaded before either saves
            es.save(&mut agg, &[CounterEvent::Set(200)]).await
        }
    });

    let result_a = handle_a.await.unwrap();
    let result_b = handle_b.await.unwrap();

    // Exactly one should succeed, one should fail.
    // Exactly one should succeed, one should fail with a conflict.
    let successes = [&result_a, &result_b].iter().filter(|r| r.is_ok()).count();
    let failures = [&result_a, &result_b].iter().filter(|r| r.is_err()).count();

    assert_eq!(successes, 1, "exactly one writer should succeed");
    assert_eq!(failures, 1, "exactly one writer should fail with conflict");

    // Conflict must surface as StoreError::Conflict, not wrapped in Adapter
    let err = match (result_a, result_b) {
        (Err(e), _) | (_, Err(e)) => e,
        _ => unreachable!("one result must be Err"),
    };
    assert!(
        matches!(err, StoreError::Conflict { .. }),
        "conflict should be StoreError::Conflict: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d3_concurrent_loads_both_succeed() {
    let es = Arc::new(EventStore::new(InMemoryStore::new(), SimpleCodec));

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Set(42)]).await.unwrap();

    let handle_a = tokio::spawn({
        let es = es.clone();
        async move {
            let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
            loaded.state().value
        }
    });
    let handle_b = tokio::spawn({
        let es = es.clone();
        async move {
            let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
            loaded.state().value
        }
    });

    let (val_a, val_b) = (handle_a.await.unwrap(), handle_b.await.unwrap());
    assert_eq!(val_a, 42);
    assert_eq!(val_b, 42);
}

#[tokio::test]
async fn d3_cross_stream_concurrent_saves_both_succeed() {
    let es = Arc::new(EventStore::new(InMemoryStore::new(), SimpleCodec));

    let handle_a = tokio::spawn({
        let es = es.clone();
        async move {
            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
            es.save(&mut agg, &[CounterEvent::Set(10)]).await
        }
    });
    let handle_b = tokio::spawn({
        let es = es.clone();
        async move {
            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(2));
            es.save(&mut agg, &[CounterEvent::Set(20)]).await
        }
    });

    handle_a.await.unwrap().unwrap();
    handle_b.await.unwrap().unwrap();

    // Both streams independent — verify isolation
    let loaded_a: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    let loaded_b: AggregateRoot<CounterAggregate> = es.load(CounterId(2)).await.unwrap();
    assert_eq!(loaded_a.state().value, 10);
    assert_eq!(loaded_b.state().value, 20);
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 4: Upcaster Chain Integration
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d4_single_upcaster_transforms_on_load() {
    let es = EventStore::with_upcaster(InMemoryStore::new(), SimpleCodec, IncrementedV1ToV2);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Set(42)]).await.unwrap();

    // Load applies upcaster (v1→v2) but payload is unchanged
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 42);
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn d4_chained_upcasters_v1_to_v3() {
    let es = EventStore::with_upcaster(InMemoryStore::new(), SimpleCodec, IncrementedV1ToV3);
    // Flow: v1 → v2 → v3 (upcaster handles both steps internally)

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(
        &mut agg,
        &[CounterEvent::Incremented, CounterEvent::Decremented],
    )
    .await
    .unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 0); // +1 -1 = 0
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

// NOTE: The chain limit test has been removed. With the new SchemaTransform API,
// each transform declares a specific (event_type, source_version) pair, making
// infinite loops structurally impossible — the pipeline validates that
// to_version > source_version, and the HList chain is finite by construction.
// The pipeline chain limit exists as a defense-in-depth measure but cannot
// be triggered through the public API with well-typed SchemaTransform impls.

#[tokio::test]
async fn d4_zero_copy_event_store_with_payload_mutating_upcaster() {
    use nexus_store::pending_envelope;

    // DeltaDoublingTransform doubles the i32 payload on read.
    // This exercises the ZeroCopyEventStore + upcaster path where
    // run_upcasters returns a NEW Vec<u8> and BorrowingCodec::decode
    // borrows from that new buffer.
    //
    // We save a "legacy" event at schema_version=1 via the raw store,
    // then load via ZeroCopyEventStore which applies the upcaster.
    let raw_store = InMemoryStore::new();

    // Write a v1 event directly (simulating a legacy event)
    let legacy = pending_envelope(Version::INITIAL)
        .event_type("Delta")
        .payload(5_i32.to_le_bytes().to_vec())
        .build_without_metadata(); // schema_version defaults to 1
    raw_store
        .append(&CounterId(1), None, &[legacy])
        .await
        .unwrap();

    // Load via ZeroCopyEventStore with upcaster — delta=5 doubled to 10
    let es =
        ZeroCopyEventStore::with_upcaster(raw_store, DeltaBorrowingCodec, DeltaDoublingUpcaster);
    let loaded: AggregateRoot<DeltaAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(
        loaded.state().total,
        10,
        "upcaster should double the legacy delta from 5 to 10"
    );
}

#[tokio::test]
async fn d4_upcaster_must_not_double_apply_to_new_events() {
    use nexus_store::pending_envelope;

    // Scenario: a legacy v1 event exists in the store, then a new event
    // is saved through the EventStore (which stamps it at schema_version=2).
    // On reload, only the legacy event should be upcasted.
    let raw_store = InMemoryStore::new();

    // Write a legacy v1 event directly (simulating pre-migration data)
    let legacy = pending_envelope(Version::INITIAL)
        .event_type("Delta")
        .payload(5_i32.to_le_bytes().to_vec())
        .build_without_metadata(); // schema_version defaults to 1
    raw_store
        .append(&CounterId(1), None, &[legacy])
        .await
        .unwrap();

    let es =
        ZeroCopyEventStore::with_upcaster(raw_store, DeltaBorrowingCodec, DeltaDoublingUpcaster);

    // Load: legacy delta=5 upcasted to 10. State total=10.
    let mut loaded: AggregateRoot<DeltaAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().total, 10);

    // Save second event: delta=3 (saved at schema_version=2, no upcasting).
    es.save(&mut loaded, &[DeltaEvent { delta: 3 }])
        .await
        .unwrap();

    // Reload: legacy event upcasted (5→10), new event untouched (3).
    // Total = 10 + 3 = 13.
    let reloaded: AggregateRoot<DeltaAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(
        reloaded.state().total,
        13,
        "new events must not be double-upcasted — expected 13 (10 + 3), got {}",
        reloaded.state().total
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 5: Version Boundary Arithmetic
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d5_first_save_uses_version_initial_as_expected() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    assert_eq!(agg.version(), Some(Version::new(1).unwrap()));

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn d5_version_consistency_through_save_load_cycles() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    for round in 1..=10u64 {
        es.save(&mut agg, &[CounterEvent::Incremented])
            .await
            .unwrap();
        assert_eq!(
            agg.version(),
            Version::new(round),
            "version mismatch after save round {round}"
        );

        let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
        assert_eq!(
            loaded.version(),
            Version::new(round),
            "loaded version mismatch at round {round}"
        );
        assert_eq!(
            loaded.state().value,
            i64::try_from(round).unwrap(),
            "state mismatch at round {round}"
        );
    }
}

#[tokio::test]
async fn d5_batch_version_tracking() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    let events = [
        CounterEvent::Incremented,
        CounterEvent::Incremented,
        CounterEvent::Set(100),
        CounterEvent::Decremented,
    ];
    es.save(&mut agg, &events).await.unwrap();
    assert_eq!(agg.version(), Some(Version::new(4).unwrap()));

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), Some(Version::new(4).unwrap()));
    assert_eq!(loaded.state().value, 99); // +1 +1 =100 -1 = 99
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 6: Aggregate Lifecycle State Machine
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d6_full_lifecycle_fresh_save_load_modify_save_load() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // 1. Fresh aggregate
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    assert_eq!(agg.state().value, 0);
    assert_eq!(agg.version(), None);

    // 2. Save events
    es.save(
        &mut agg,
        &[CounterEvent::Set(10), CounterEvent::Incremented],
    )
    .await
    .unwrap();
    assert_eq!(agg.version(), Some(Version::new(2).unwrap()));

    // 3. Load from store
    let mut loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 11);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));

    // 4. Save more events
    es.save(&mut loaded, &[CounterEvent::Set(0)]).await.unwrap();
    assert_eq!(loaded.version(), Some(Version::new(3).unwrap()));

    // 5. Final load — verify complete state
    let final_agg: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(final_agg.state().value, 0);
    assert_eq!(final_agg.version(), Some(Version::new(3).unwrap()));
}

#[tokio::test]
async fn d6_ten_round_modify_save_load_cycles() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    for round in 0..10 {
        es.save(&mut agg, &[CounterEvent::Incremented])
            .await
            .unwrap();

        // Reload between each round
        agg = es.load(CounterId(1)).await.unwrap();
        assert_eq!(agg.state().value, (round + 1) as i64);
    }

    assert_eq!(agg.state().value, 10);
    assert_eq!(agg.version(), Some(Version::new(10).unwrap()));
}

#[tokio::test]
async fn d6_state_determinism_same_events_same_state() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(
        &mut agg,
        &[
            CounterEvent::Set(100),
            CounterEvent::Decremented,
            CounterEvent::Decremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Load twice — must produce identical state (deterministic replay)
    let load1: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    let load2: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();

    assert_eq!(load1.state(), load2.state());
    assert_eq!(load1.version(), load2.version());
    assert_eq!(load1.state().value, 99); // 100 -1 -1 +1
}

#[tokio::test]
async fn d6_load_after_save_has_no_pending_state() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    // Loaded aggregate version should match
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 7: Repository Contract Verification
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d7_save_empty_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    // Save with empty events — should be a no-op
    es.save(&mut agg, &[]).await.unwrap();
    assert_eq!(agg.version(), None);
}

#[tokio::test]
async fn d7_save_advances_version() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    es.save(
        &mut agg,
        &[CounterEvent::Incremented, CounterEvent::Decremented],
    )
    .await
    .unwrap();
    assert_eq!(agg.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn d7_load_nonexistent_stream_returns_fresh_aggregate() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();

    assert_eq!(loaded.version(), None);
    assert_eq!(loaded.state(), &CounterState::default());
}

#[tokio::test]
async fn d7_repository_preserves_event_ordering() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    es.save(
        &mut agg,
        &[
            CounterEvent::Set(100),
            CounterEvent::Decremented,
            CounterEvent::Set(0),
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(
        loaded.state().value,
        1,
        "events must be replayed in save order"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 8: Error Propagation Paths
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d8_codec_encode_error_is_store_error_codec() {
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    match es.save(&mut agg, &[CounterEvent::Incremented]).await {
        Err(StoreError::Codec(inner)) => {
            assert!(
                inner.to_string().contains("encode limit"),
                "inner error should describe encode failure: {inner}"
            );
        }
        other => panic!("expected StoreError::Codec, got: {other:?}"),
    }
}

#[tokio::test]
async fn d8_codec_decode_error_is_store_error_codec() {
    let es = EventStore::new(InMemoryStore::new(), FailAfterNDecodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    let result: Result<AggregateRoot<CounterAggregate>, _> = es.load(CounterId(1)).await;
    match result {
        Err(StoreError::Codec(inner)) => {
            assert!(inner.to_string().contains("decode limit"));
        }
        other => panic!("expected StoreError::Codec, got: {other:?}"),
    }
}

#[tokio::test]
async fn d8_concurrency_conflict_should_be_store_error_conflict_not_adapter() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg1 = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg1, &[CounterEvent::Incremented])
        .await
        .unwrap();

    let mut copy_a: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    let mut copy_b: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();

    es.save(&mut copy_a, &[CounterEvent::Incremented])
        .await
        .unwrap();
    let err = es
        .save(&mut copy_b, &[CounterEvent::Decremented])
        .await
        .unwrap_err();

    assert!(
        matches!(err, StoreError::Conflict { .. }),
        "concurrency conflict must be StoreError::Conflict, not wrapped in Adapter. Got: {err:?}"
    );
}

#[tokio::test]
async fn d8_rehydration_limit_exceeded_is_store_error_kernel() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save 6 events (3 + 3) using TinyAggregate (MAX_REHYDRATION_EVENTS=5)
    let mut agg = AggregateRoot::<TinyAggregate>::new(CounterId(1));
    es.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    let mut agg: AggregateRoot<TinyAggregate> = es.load(CounterId(1)).await.unwrap();
    es.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Load 6 events with MAX_REHYDRATION_EVENTS=5 → fails at version 6
    let result: Result<AggregateRoot<TinyAggregate>, _> = es.load(CounterId(1)).await;
    match result {
        Err(StoreError::Kernel(KernelError::RehydrationLimitExceeded { max, .. })) => {
            assert_eq!(max, 5);
        }
        other => panic!("expected RehydrationLimitExceeded, got: {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 9: Property-Based Algebraic Laws
// ═══════════════════════════════════════════════════════════════════════════

fn counter_event_strategy() -> impl Strategy<Value = CounterEvent> {
    prop_oneof![
        Just(CounterEvent::Incremented),
        Just(CounterEvent::Decremented),
        (-1000i64..1000).prop_map(CounterEvent::Set),
    ]
}

proptest! {
    #[test]
    fn d9_roundtrip_identity_save_then_load(
        events in prop::collection::vec(counter_event_strategy(), 1..20)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

            // Compute expected state
            let expected_value = events.iter().fold(0i64, |acc, e| match e {
                CounterEvent::Incremented => acc + 1,
                CounterEvent::Decremented => acc - 1,
                CounterEvent::Set(v) => *v,
            });

            es.save(&mut agg, &events).await.unwrap();

            let loaded: AggregateRoot<CounterAggregate> =
                es.load(CounterId(1)).await.unwrap();
            prop_assert_eq!(loaded.state().value, expected_value);
            prop_assert_eq!(loaded.version().unwrap().as_u64(), events.len() as u64);
            Ok(())
        })?;
    }

    #[test]
    fn d9_event_order_preserved(
        events in prop::collection::vec(counter_event_strategy(), 2..15)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

            let expected_value = events.iter().fold(0i64, |acc, e| match e {
                CounterEvent::Incremented => acc + 1,
                CounterEvent::Decremented => acc - 1,
                CounterEvent::Set(v) => *v,
            });

            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
            es.save(&mut agg, &events).await.unwrap();

            let loaded: AggregateRoot<CounterAggregate> =
                es.load(CounterId(1)).await.unwrap();
            prop_assert_eq!(loaded.state().value, expected_value);
            Ok(())
        })?;
    }

    #[test]
    fn d9_state_is_pure_function_of_events(
        events in prop::collection::vec(counter_event_strategy(), 1..10)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
            es.save(&mut agg, &events).await.unwrap();

            // Load twice — must produce identical state (deterministic replay)
            let load1: AggregateRoot<CounterAggregate> =
                es.load(CounterId(1)).await.unwrap();
            let load2: AggregateRoot<CounterAggregate> =
                es.load(CounterId(1)).await.unwrap();

            prop_assert_eq!(load1.state().value, load2.state().value);
            prop_assert_eq!(load1.version(), load2.version());
            Ok(())
        })?;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 10: Resource Limits
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d10_large_batch_500_events_save_and_load() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    let events: Vec<CounterEvent> = (0..500).map(|_| CounterEvent::Incremented).collect();
    es.save(&mut agg, &events).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 500);
    assert_eq!(loaded.version(), Some(Version::new(500).unwrap()));
}

#[tokio::test]
async fn d10_max_rehydration_events_boundary() {
    // TinyAggregate: MAX_REHYDRATION_EVENTS = 5
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save exactly 5 events (3 + 2 via reload)
    let mut agg = AggregateRoot::<TinyAggregate>::new(CounterId(1));
    es.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    let mut agg: AggregateRoot<TinyAggregate> = es.load(CounterId(1)).await.unwrap();
    es.save(
        &mut agg,
        &[CounterEvent::Incremented, CounterEvent::Incremented],
    )
    .await
    .unwrap();

    // Load 5 events — should succeed (version 5 is not > 5)
    let loaded: AggregateRoot<TinyAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 5);
    assert_eq!(loaded.version(), Some(Version::new(5).unwrap()));
}

// NOTE: MAX_UNCOMMITTED has been removed from the API. AggregateRoot no longer
// buffers uncommitted events — events are passed directly to Repository::save().

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 11: InMemoryStore Edge Cases
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d11_schema_version_always_one() {
    use nexus_store::pending_envelope;
    use nexus_store::stream::EventStream;

    let store = InMemoryStore::new();

    let envelopes = [
        pending_envelope(Version::INITIAL)
            .event_type("Incremented")
            .payload(b"inc".to_vec())
            .build_without_metadata(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("Set")
            .payload(b"set:42".to_vec())
            .build_without_metadata(),
    ];
    store.append(&CounterId(1), None, &envelopes).await.unwrap();

    let mut stream = store
        .read_stream(&CounterId(1), Version::INITIAL)
        .await
        .unwrap();
    let env1 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        env1.schema_version(),
        1,
        "default schema_version should be 1"
    );
    let env2 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        env2.schema_version(),
        1,
        "default schema_version should be 1"
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn d11_save_and_load_with_typed_id() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    let result = es.save(&mut agg, &[CounterEvent::Incremented]).await;
    assert!(result.is_ok(), "save with typed id should be accepted");

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 1);
}

#[tokio::test]
async fn d11_stream_isolation_different_streams_independent() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg_a = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg_a, &[CounterEvent::Set(100)])
        .await
        .unwrap();

    let mut agg_b = AggregateRoot::<CounterAggregate>::new(CounterId(2));
    es.save(
        &mut agg_b,
        &[CounterEvent::Set(200), CounterEvent::Incremented],
    )
    .await
    .unwrap();

    let loaded_a: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    let loaded_b: AggregateRoot<CounterAggregate> = es.load(CounterId(2)).await.unwrap();

    assert_eq!(loaded_a.state().value, 100);
    assert_eq!(loaded_a.version(), Some(Version::new(1).unwrap()));
    assert_eq!(loaded_b.state().value, 201);
    assert_eq!(loaded_b.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn d11_append_empty_batch_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&mut agg, &[]).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), None);
}

#[tokio::test]
async fn d11_multiple_event_types_in_single_stream() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    es.save(
        &mut agg,
        &[
            CounterEvent::Set(50),
            CounterEvent::Incremented,
            CounterEvent::Decremented,
            CounterEvent::Decremented,
            CounterEvent::Set(-10),
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, -9); // 50 +1 -1 -1 = -10 +1 = -9
    assert_eq!(loaded.version(), Some(Version::new(6).unwrap()));
}
