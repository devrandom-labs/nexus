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
use nexus_store::raw::RawEventStore;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;
use nexus_store::upcaster::EventUpcaster;
use proptest::prelude::*;

fn sid(s: &str) -> StreamId {
    StreamId::from_persisted(s).unwrap()
}

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

#[derive(Default, Debug, PartialEq)]
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

    fn name(&self) -> &'static str {
        "Counter"
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
    const MAX_REHYDRATION_EVENTS: usize = 5;
    const MAX_UNCOMMITTED: usize = 3;
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

#[derive(Default, Debug, PartialEq)]
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

    fn name(&self) -> &'static str {
        "Delta"
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

/// Identity upcaster: bumps version from 1→2 without changing payload.
struct V1ToV2Upcaster;

impl EventUpcaster for V1ToV2Upcaster {
    fn can_upcast(&self, _event_type: &str, schema_version: u32) -> bool {
        schema_version == 1
    }

    fn upcast(
        &self,
        event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        (event_type.to_owned(), 2, payload.to_vec())
    }
}

/// Second-stage upcaster: bumps version from 2→3 without changing payload.
struct V2ToV3Upcaster;

impl EventUpcaster for V2ToV3Upcaster {
    fn can_upcast(&self, _event_type: &str, schema_version: u32) -> bool {
        schema_version == 2
    }

    fn upcast(
        &self,
        event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        (event_type.to_owned(), 3, payload.to_vec())
    }
}

/// Upcaster that always matches and always advances — triggers chain limit.
struct AlwaysUpcastsUpcaster;

impl EventUpcaster for AlwaysUpcastsUpcaster {
    fn can_upcast(&self, _event_type: &str, _schema_version: u32) -> bool {
        true
    }

    fn upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        (event_type.to_owned(), schema_version + 1, payload.to_vec())
    }
}

/// Upcaster that doubles the i32 delta — actually mutates payload bytes.
struct DeltaDoublingUpcaster;

impl EventUpcaster for DeltaDoublingUpcaster {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        event_type == "Delta" && schema_version == 1
    }

    fn upcast(
        &self,
        event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        let delta = i32::from_le_bytes(payload[0..4].try_into().unwrap());
        let doubled = delta * 2;
        (event_type.to_owned(), 2, doubled.to_le_bytes().to_vec())
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
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);

    let result = es.save(&sid("s1"), &mut agg).await;
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
    agg.apply(CounterEvent::Incremented);

    let result = es.save(&sid("s1"), &mut agg).await;
    assert!(matches!(result.unwrap_err(), StoreError::Codec(_)));
}

#[tokio::test]
async fn d1_decode_failure_mid_stream_returns_codec_error() {
    // FailAfterNDecodesCodec: encode always succeeds, decode fails after N calls.
    // Save 3 events (encodes fine), then load (2nd decode fails).
    let es = EventStore::new(InMemoryStore::new(), FailAfterNDecodesCodec::new(1));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut agg).await.unwrap();

    // Load — first decode succeeds, second fails
    let result: Result<AggregateRoot<CounterAggregate>, _> =
        es.load(&sid("s1"), CounterId(1)).await;
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
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut agg).await.unwrap(); // encode works fine

    let result: Result<AggregateRoot<CounterAggregate>, _> =
        es.load(&sid("s1"), CounterId(1)).await;
    assert!(matches!(result.unwrap_err(), StoreError::Codec(_)));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 2: Save Atomicity — data loss on failed save
//
// BUG: save_with_encoder calls take_uncommitted_events() BEFORE encoding
// or appending. If either step fails, events are permanently lost and the
// aggregate's version is incorrectly advanced. The aggregate becomes
// "wedged" — unable to save because its version doesn't match the store.
//
// These tests assert the CORRECT expected behavior. They FAIL until fixed.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d2_failed_encode_must_not_consume_uncommitted_events() {
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);

    assert_eq!(agg.version(), Version::INITIAL);
    assert_eq!(agg.current_version(), Version::from_persisted(2));

    // Save fails (encode error)
    let result = es.save(&sid("s1"), &mut agg).await;
    assert!(result.is_err());

    // CORRECT: version must NOT advance on failed save
    assert_eq!(
        agg.version(),
        Version::INITIAL,
        "version must remain INITIAL after failed save — no events were persisted"
    );

    // CORRECT: uncommitted events must still be available for retry
    assert_eq!(
        agg.current_version(),
        Version::from_persisted(2),
        "uncommitted events must survive a failed save"
    );
}

#[tokio::test]
async fn d2_failed_save_must_preserve_uncommitted_events() {
    let es = EventStore::new(InMemoryStore::new(), FailAfterNEncodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);

    let _ = es.save(&sid("s1"), &mut agg).await; // fails

    // CORRECT: uncommitted events must survive for retry
    let events = agg.take_uncommitted_events();
    assert_eq!(
        events.len(),
        1,
        "uncommitted events must survive a failed save — got {} instead of 1",
        events.len()
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

    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Incremented);

    // First save fails
    let result = es.save(&sid("s1"), &mut agg).await;
    assert!(result.is_err());

    // Fix the codec
    fail_flag.store(false, Ordering::SeqCst);

    // CORRECT: retry must succeed — same events should persist
    let retry = es.save(&sid("s1"), &mut agg).await;
    assert!(
        retry.is_ok(),
        "aggregate must be retryable after a failed save — got: {:?}",
        retry.unwrap_err()
    );

    // CORRECT: loading must return the persisted state
    let loaded: AggregateRoot<CounterAggregate> = es.load(&sid("s1"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 2);
    assert_eq!(loaded.version(), Version::from_persisted(2));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 3: Concurrent Access
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d3_concurrent_saves_one_wins_one_conflicts() {
    let es = Arc::new(EventStore::new(InMemoryStore::new(), SimpleCodec));
    let stream = sid("race-stream");

    // Setup: one event in the stream
    let mut setup = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    setup.apply(CounterEvent::Set(0));
    es.save(&stream, &mut setup).await.unwrap();

    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    // Task A: load, wait for B, save
    let handle_a = tokio::spawn({
        let es = es.clone();
        let barrier = barrier.clone();
        let stream = stream.clone();
        async move {
            let mut agg: AggregateRoot<CounterAggregate> =
                es.load(&stream, CounterId(1)).await.unwrap();
            barrier.wait().await; // both loaded before either saves
            agg.apply(CounterEvent::Set(100));
            es.save(&stream, &mut agg).await
        }
    });

    // Task B: load, wait for A, save
    let handle_b = tokio::spawn({
        let es = es.clone();
        let barrier = barrier.clone();
        let stream = stream.clone();
        async move {
            let mut agg: AggregateRoot<CounterAggregate> =
                es.load(&stream, CounterId(1)).await.unwrap();
            barrier.wait().await; // both loaded before either saves
            agg.apply(CounterEvent::Set(200));
            es.save(&stream, &mut agg).await
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
    agg.apply(CounterEvent::Set(42));
    es.save(&sid("s1"), &mut agg).await.unwrap();

    let handle_a = tokio::spawn({
        let es = es.clone();
        async move {
            let loaded: AggregateRoot<CounterAggregate> =
                es.load(&sid("s1"), CounterId(1)).await.unwrap();
            loaded.state().value
        }
    });
    let handle_b = tokio::spawn({
        let es = es.clone();
        async move {
            let loaded: AggregateRoot<CounterAggregate> =
                es.load(&sid("s1"), CounterId(1)).await.unwrap();
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
            agg.apply(CounterEvent::Set(10));
            es.save(&sid("stream-a"), &mut agg).await
        }
    });
    let handle_b = tokio::spawn({
        let es = es.clone();
        async move {
            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(2));
            agg.apply(CounterEvent::Set(20));
            es.save(&sid("stream-b"), &mut agg).await
        }
    });

    handle_a.await.unwrap().unwrap();
    handle_b.await.unwrap().unwrap();

    // Both streams independent — verify isolation
    let loaded_a: AggregateRoot<CounterAggregate> =
        es.load(&sid("stream-a"), CounterId(1)).await.unwrap();
    let loaded_b: AggregateRoot<CounterAggregate> =
        es.load(&sid("stream-b"), CounterId(2)).await.unwrap();
    assert_eq!(loaded_a.state().value, 10);
    assert_eq!(loaded_b.state().value, 20);
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 4: Upcaster Chain Integration
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d4_single_upcaster_transforms_on_load() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec).with_upcaster(V1ToV2Upcaster);
    let stream = sid("upcaster-1");

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Set(42));
    es.save(&stream, &mut agg).await.unwrap();

    // Load applies upcaster (v1→v2) but payload is unchanged
    let loaded: AggregateRoot<CounterAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 42);
    assert_eq!(loaded.version(), Version::from_persisted(1));
}

#[tokio::test]
async fn d4_chained_upcasters_v1_to_v3() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec)
        .with_upcaster(V2ToV3Upcaster)
        .with_upcaster(V1ToV2Upcaster);
    // Chain order: V1ToV2 is checked first (prepended last).
    // Flow: v1 → V1ToV2 → v2 → V2ToV3 → v3

    let stream = sid("chain-stream");
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Decremented);
    es.save(&stream, &mut agg).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 0); // +1 -1 = 0
    assert_eq!(loaded.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn d4_chain_limit_exceeded_returns_error_on_load() {
    let es =
        EventStore::new(InMemoryStore::new(), SimpleCodec).with_upcaster(AlwaysUpcastsUpcaster);
    let stream = sid("infinite-chain");

    // Save one event
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    es.save(&stream, &mut agg).await.unwrap();

    // Load triggers infinite upcaster chain → hits 100-iteration limit
    let result: Result<AggregateRoot<CounterAggregate>, _> = es.load(&stream, CounterId(1)).await;
    assert!(
        matches!(result, Err(StoreError::Codec(_))),
        "chain limit exceeded should surface as StoreError::Codec"
    );
}

#[tokio::test]
async fn d4_zero_copy_event_store_with_payload_mutating_upcaster() {
    use nexus_store::pending_envelope;

    // DeltaDoublingUpcaster doubles the i32 payload on read.
    // This exercises the ZeroCopyEventStore + upcaster path where
    // run_upcasters returns a NEW Vec<u8> and BorrowingCodec::decode
    // borrows from that new buffer.
    //
    // We save a "legacy" event at schema_version=1 via the raw store,
    // then load via ZeroCopyEventStore which applies the upcaster.
    let raw_store = InMemoryStore::new();

    // Write a v1 event directly (simulating a legacy event)
    let legacy = pending_envelope(sid("zc-upcast"))
        .version(Version::from_persisted(1))
        .event_type("Delta")
        .payload(5_i32.to_le_bytes().to_vec())
        .build_without_metadata(); // schema_version defaults to 1
    raw_store
        .append(&sid("zc-upcast"), Version::INITIAL, &[legacy])
        .await
        .unwrap();

    // Load via ZeroCopyEventStore with upcaster — delta=5 doubled to 10
    let es = ZeroCopyEventStore::new(raw_store, DeltaBorrowingCodec)
        .with_upcaster(DeltaDoublingUpcaster);
    let loaded: AggregateRoot<DeltaAggregate> =
        es.load(&sid("zc-upcast"), CounterId(1)).await.unwrap();
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
    let stream = sid("double-upcast");

    // Write a legacy v1 event directly (simulating pre-migration data)
    let legacy = pending_envelope(stream.clone())
        .version(Version::from_persisted(1))
        .event_type("Delta")
        .payload(5_i32.to_le_bytes().to_vec())
        .build_without_metadata(); // schema_version defaults to 1
    raw_store
        .append(&stream, Version::INITIAL, &[legacy])
        .await
        .unwrap();

    let es = ZeroCopyEventStore::new(raw_store, DeltaBorrowingCodec)
        .with_upcaster(DeltaDoublingUpcaster);

    // Load: legacy delta=5 upcasted to 10. State total=10.
    let mut loaded: AggregateRoot<DeltaAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().total, 10);

    // Apply second event: delta=3 (saved at schema_version=2, no upcasting).
    loaded.apply(DeltaEvent { delta: 3 });
    es.save(&stream, &mut loaded).await.unwrap();

    // Reload: legacy event upcasted (5→10), new event untouched (3).
    // Total = 10 + 3 = 13.
    let reloaded: AggregateRoot<DeltaAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
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
    agg.apply(CounterEvent::Incremented);

    // First event has version 1. expected_version = 1-1 = 0 (INITIAL).
    es.save(&sid("s1"), &mut agg).await.unwrap();

    // Verify version after save
    assert_eq!(agg.version(), Version::from_persisted(1));

    // Verify correct readback
    let loaded: AggregateRoot<CounterAggregate> = es.load(&sid("s1"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), Version::from_persisted(1));
}

#[tokio::test]
async fn d5_version_consistency_through_save_load_cycles() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    for round in 1..=10u64 {
        agg.apply(CounterEvent::Incremented);
        es.save(&sid("s1"), &mut agg).await.unwrap();
        assert_eq!(
            agg.version(),
            Version::from_persisted(round),
            "version mismatch after save round {round}"
        );

        let loaded: AggregateRoot<CounterAggregate> =
            es.load(&sid("s1"), CounterId(1)).await.unwrap();
        assert_eq!(
            loaded.version(),
            Version::from_persisted(round),
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
    // Save multiple events in one batch, verify versions are sequential
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    agg.apply(CounterEvent::Incremented); // v1
    agg.apply(CounterEvent::Incremented); // v2
    agg.apply(CounterEvent::Set(100)); // v3
    agg.apply(CounterEvent::Decremented); // v4

    assert_eq!(agg.current_version(), Version::from_persisted(4));
    es.save(&sid("s1"), &mut agg).await.unwrap();
    assert_eq!(agg.version(), Version::from_persisted(4));

    let loaded: AggregateRoot<CounterAggregate> = es.load(&sid("s1"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), Version::from_persisted(4));
    assert_eq!(loaded.state().value, 99); // +1 +1 =100 -1 = 99
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 6: Aggregate Lifecycle State Machine
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d6_full_lifecycle_fresh_save_load_modify_save_load() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let stream = sid("lifecycle");

    // 1. Fresh aggregate
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    assert_eq!(agg.state().value, 0);
    assert_eq!(agg.version(), Version::INITIAL);

    // 2. Apply events and save
    agg.apply(CounterEvent::Set(10));
    agg.apply(CounterEvent::Incremented);
    es.save(&stream, &mut agg).await.unwrap();
    assert_eq!(agg.version(), Version::from_persisted(2));

    // 3. Load from store
    let mut loaded: AggregateRoot<CounterAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 11);
    assert_eq!(loaded.version(), Version::from_persisted(2));

    // 4. Modify and save again
    loaded.apply(CounterEvent::Set(0));
    es.save(&stream, &mut loaded).await.unwrap();
    assert_eq!(loaded.version(), Version::from_persisted(3));

    // 5. Final load — verify complete state
    let final_agg: AggregateRoot<CounterAggregate> = es.load(&stream, CounterId(1)).await.unwrap();
    assert_eq!(final_agg.state().value, 0);
    assert_eq!(final_agg.version(), Version::from_persisted(3));
}

#[tokio::test]
async fn d6_ten_round_modify_save_load_cycles() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let stream = sid("ten-rounds");

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    for round in 0..10 {
        agg.apply(CounterEvent::Incremented);
        es.save(&stream, &mut agg).await.unwrap();

        // Reload between each round
        agg = es.load(&stream, CounterId(1)).await.unwrap();
        assert_eq!(agg.state().value, (round + 1) as i64);
    }

    assert_eq!(agg.state().value, 10);
    assert_eq!(agg.version(), Version::from_persisted(10));
}

#[tokio::test]
async fn d6_state_determinism_same_events_same_state() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save a specific event sequence
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Set(100));
    agg.apply(CounterEvent::Decremented);
    agg.apply(CounterEvent::Decremented);
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("det"), &mut agg).await.unwrap();

    // Load twice — must produce identical state
    let load1: AggregateRoot<CounterAggregate> = es.load(&sid("det"), CounterId(1)).await.unwrap();
    let load2: AggregateRoot<CounterAggregate> = es.load(&sid("det"), CounterId(2)).await.unwrap();

    assert_eq!(load1.state(), load2.state());
    assert_eq!(load1.version(), load2.version());
    assert_eq!(load1.state().value, 99); // 100 -1 -1 +1
}

#[tokio::test]
async fn d6_load_after_save_has_no_uncommitted_events() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut agg).await.unwrap();

    let mut loaded: AggregateRoot<CounterAggregate> =
        es.load(&sid("s1"), CounterId(1)).await.unwrap();
    // Loaded aggregate should have no uncommitted events
    let events = loaded.take_uncommitted_events();
    assert!(events.is_empty());
    // Version shouldn't change from taking empty events
    assert_eq!(loaded.version(), Version::from_persisted(1));
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 7: Repository Contract Verification
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d7_save_empty_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    // Save with no uncommitted events — should be a no-op
    es.save(&sid("s1"), &mut agg).await.unwrap();
    assert_eq!(agg.version(), Version::INITIAL);
}

#[tokio::test]
async fn d7_save_drains_uncommitted_events() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Decremented);
    assert_eq!(agg.current_version(), Version::from_persisted(2));

    es.save(&sid("s1"), &mut agg).await.unwrap();

    // After save, version = current_version (events drained)
    assert_eq!(agg.version(), agg.current_version());
    assert_eq!(agg.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn d7_load_nonexistent_stream_returns_fresh_aggregate() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let loaded: AggregateRoot<CounterAggregate> =
        es.load(&sid("does-not-exist"), CounterId(1)).await.unwrap();

    assert_eq!(loaded.version(), Version::INITIAL);
    assert_eq!(loaded.state(), &CounterState::default());
}

#[tokio::test]
async fn d7_repository_preserves_event_ordering() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    // Apply events in a specific order that only produces the right state
    // if events are replayed in the same order.
    agg.apply(CounterEvent::Set(100)); // v1: set to 100
    agg.apply(CounterEvent::Decremented); // v2: 99
    agg.apply(CounterEvent::Set(0)); // v3: set to 0
    agg.apply(CounterEvent::Incremented); // v4: 1
    es.save(&sid("ord"), &mut agg).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(&sid("ord"), CounterId(1)).await.unwrap();
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
    agg.apply(CounterEvent::Incremented);

    match es.save(&sid("s1"), &mut agg).await {
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
    // FailAfterNDecodesCodec: encode works, decode fails immediately
    let es = EventStore::new(InMemoryStore::new(), FailAfterNDecodesCodec::new(0));
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut agg).await.unwrap(); // encode fine

    let result: Result<AggregateRoot<CounterAggregate>, _> =
        es.load(&sid("s1"), CounterId(1)).await;
    match result {
        Err(StoreError::Codec(inner)) => {
            assert!(inner.to_string().contains("decode limit"));
        }
        other => panic!("expected StoreError::Codec, got: {other:?}"),
    }
}

#[tokio::test]
async fn d8_concurrency_conflict_should_be_store_error_conflict_not_adapter() {
    // AppendError separates conflicts from adapter errors, so EventStore maps
    // AppendError::Conflict directly to StoreError::Conflict.
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    let mut agg1 = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg1.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut agg1).await.unwrap();

    let mut copy_a: AggregateRoot<CounterAggregate> =
        es.load(&sid("s1"), CounterId(1)).await.unwrap();
    let mut copy_b: AggregateRoot<CounterAggregate> =
        es.load(&sid("s1"), CounterId(1)).await.unwrap();

    copy_a.apply(CounterEvent::Incremented);
    es.save(&sid("s1"), &mut copy_a).await.unwrap();

    copy_b.apply(CounterEvent::Decremented);
    let err = es.save(&sid("s1"), &mut copy_b).await.unwrap_err();

    // CORRECT: conflict must surface as StoreError::Conflict, not Adapter
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
    agg.apply(CounterEvent::Incremented); // v1
    agg.apply(CounterEvent::Incremented); // v2
    agg.apply(CounterEvent::Incremented); // v3
    es.save(&sid("tiny"), &mut agg).await.unwrap();

    let mut agg: AggregateRoot<TinyAggregate> = es.load(&sid("tiny"), CounterId(1)).await.unwrap();
    agg.apply(CounterEvent::Incremented); // v4
    agg.apply(CounterEvent::Incremented); // v5
    agg.apply(CounterEvent::Incremented); // v6
    es.save(&sid("tiny"), &mut agg).await.unwrap();

    // Load 6 events with MAX_REHYDRATION_EVENTS=5 → fails at version 6
    let result: Result<AggregateRoot<TinyAggregate>, _> = es.load(&sid("tiny"), CounterId(1)).await;
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

            for event in &events {
                agg.apply(event.clone());
            }
            let state_before_save = agg.state().value;
            es.save(&sid("rt"), &mut agg).await.unwrap();

            let loaded: AggregateRoot<CounterAggregate> =
                es.load(&sid("rt"), CounterId(1)).await.unwrap();
            prop_assert_eq!(loaded.state().value, state_before_save);
            prop_assert_eq!(loaded.version().as_u64(), events.len() as u64);
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

            // Apply events in order and compute expected state
            let expected_value = events.iter().fold(0i64, |acc, e| match e {
                CounterEvent::Incremented => acc + 1,
                CounterEvent::Decremented => acc - 1,
                CounterEvent::Set(v) => *v,
            });

            let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
            for event in &events {
                agg.apply(event.clone());
            }
            es.save(&sid("ord"), &mut agg).await.unwrap();

            let loaded: AggregateRoot<CounterAggregate> =
                es.load(&sid("ord"), CounterId(1)).await.unwrap();
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
            for event in &events {
                agg.apply(event.clone());
            }
            es.save(&sid("pure"), &mut agg).await.unwrap();

            // Load twice — must produce identical state (deterministic replay)
            let load1: AggregateRoot<CounterAggregate> =
                es.load(&sid("pure"), CounterId(1)).await.unwrap();
            let load2: AggregateRoot<CounterAggregate> =
                es.load(&sid("pure"), CounterId(2)).await.unwrap();

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

    for _ in 0..500 {
        agg.apply(CounterEvent::Incremented);
    }
    es.save(&sid("big"), &mut agg).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> = es.load(&sid("big"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 500);
    assert_eq!(loaded.version(), Version::from_persisted(500));
}

#[tokio::test]
async fn d10_max_rehydration_events_boundary() {
    // TinyAggregate: MAX_REHYDRATION_EVENTS = 5
    // Exactly 5 events should succeed; 6 should fail.
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save exactly 5 events (3 + 2 via reload)
    let mut agg = AggregateRoot::<TinyAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented); // v1
    agg.apply(CounterEvent::Incremented); // v2
    agg.apply(CounterEvent::Incremented); // v3
    es.save(&sid("boundary"), &mut agg).await.unwrap();

    let mut agg: AggregateRoot<TinyAggregate> =
        es.load(&sid("boundary"), CounterId(1)).await.unwrap();
    agg.apply(CounterEvent::Incremented); // v4
    agg.apply(CounterEvent::Incremented); // v5
    es.save(&sid("boundary"), &mut agg).await.unwrap();

    // Load 5 events — should succeed (version 5 is not > 5)
    let loaded: AggregateRoot<TinyAggregate> =
        es.load(&sid("boundary"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 5);
    assert_eq!(loaded.version(), Version::from_persisted(5));
}

#[test]
#[should_panic(expected = "Uncommitted event limit reached")]
fn d10_max_uncommitted_panics() {
    // TinyAggregate: MAX_UNCOMMITTED = 3
    let mut agg = AggregateRoot::<TinyAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented); // 1
    agg.apply(CounterEvent::Incremented); // 2
    agg.apply(CounterEvent::Incremented); // 3
    agg.apply(CounterEvent::Incremented); // 4 — PANICS
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension 11: InMemoryStore Edge Cases
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn d11_schema_version_always_one() {
    use nexus_store::pending_envelope;
    use nexus_store::stream::EventStream;

    // InMemoryStore preserves the schema_version from PendingEnvelope.
    // Verify by writing raw envelopes and reading them back.
    let store = InMemoryStore::new();

    let envelopes = [
        pending_envelope(sid("sv"))
            .version(Version::from_persisted(1))
            .event_type("Incremented")
            .payload(b"inc".to_vec())
            .build_without_metadata(),
        pending_envelope(sid("sv"))
            .version(Version::from_persisted(2))
            .event_type("Set")
            .payload(b"set:42".to_vec())
            .build_without_metadata(),
    ];
    store
        .append(&sid("sv"), Version::INITIAL, &envelopes)
        .await
        .unwrap();

    // Read raw stream and verify schema versions
    let mut stream = store
        .read_stream(&sid("sv"), Version::INITIAL)
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
async fn d11_stream_id_rejects_empty_string() {
    // StreamId::from_persisted rejects empty strings at construction time.
    // This is the desired compile-time safety improvement from the StreamId type.
    let result = StreamId::from_persisted("");
    assert!(result.is_err(), "StreamId should reject empty string");

    // Valid stream_id still works
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent::Incremented);
    let result = es.save(&sid("valid-stream"), &mut agg).await;
    assert!(result.is_ok(), "valid stream_id should be accepted");

    let loaded: AggregateRoot<CounterAggregate> =
        es.load(&sid("valid-stream"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 1);
}

#[tokio::test]
async fn d11_stream_isolation_different_streams_independent() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save to stream A
    let mut agg_a = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg_a.apply(CounterEvent::Set(100));
    es.save(&sid("stream-a"), &mut agg_a).await.unwrap();

    // Save to stream B
    let mut agg_b = AggregateRoot::<CounterAggregate>::new(CounterId(2));
    agg_b.apply(CounterEvent::Set(200));
    agg_b.apply(CounterEvent::Incremented);
    es.save(&sid("stream-b"), &mut agg_b).await.unwrap();

    // Load each — must be independent
    let loaded_a: AggregateRoot<CounterAggregate> =
        es.load(&sid("stream-a"), CounterId(1)).await.unwrap();
    let loaded_b: AggregateRoot<CounterAggregate> =
        es.load(&sid("stream-b"), CounterId(2)).await.unwrap();

    assert_eq!(loaded_a.state().value, 100);
    assert_eq!(loaded_a.version(), Version::from_persisted(1));
    assert_eq!(loaded_b.state().value, 201);
    assert_eq!(loaded_b.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn d11_append_empty_batch_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);

    // Save with no events — should not create the stream
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    es.save(&sid("empty-batch"), &mut agg).await.unwrap();

    // Loading should return fresh aggregate
    let loaded: AggregateRoot<CounterAggregate> =
        es.load(&sid("empty-batch"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.version(), Version::INITIAL);
}

#[tokio::test]
async fn d11_multiple_event_types_in_single_stream() {
    // Verify mixed event types in a single stream work correctly
    let es = EventStore::new(InMemoryStore::new(), SimpleCodec);
    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));

    agg.apply(CounterEvent::Set(50));
    agg.apply(CounterEvent::Incremented);
    agg.apply(CounterEvent::Decremented);
    agg.apply(CounterEvent::Decremented);
    agg.apply(CounterEvent::Set(-10));
    agg.apply(CounterEvent::Incremented);
    es.save(&sid("mixed"), &mut agg).await.unwrap();

    let loaded: AggregateRoot<CounterAggregate> =
        es.load(&sid("mixed"), CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, -9); // 50 +1 -1 -1 = -10 +1 = -9
    assert_eq!(loaded.version(), Version::from_persisted(6));
}
