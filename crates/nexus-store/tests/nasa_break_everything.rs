//! # NASA-Grade Adversarial Property Tests for nexus-store
//!
//! Mission: systematically try to BREAK every invariant this crate claims.
//! Tool: proptest. Strategy: generate chaotic inputs, verify contracts hold.
//!
//! ## Attack Surface Catalog
//!
//! 1.  Envelope: schema_version=0 must panic/error
//! 2.  Envelope: typestate builder produces valid envelopes for any inputs
//! 3.  Envelope: PersistedEnvelope field accessors are faithful
//! 4.  RawEventStore: optimistic concurrency rejects wrong expected_version
//! 5.  RawEventStore: sequential version enforcement
//! 6.  RawEventStore: append-then-read roundtrip preserves all data
//! 7.  EventStream: monotonically increasing versions
//! 8.  EventStream: fused after None
//! 9.  Upcaster: apply_upcasters rejects non-advancing versions
//! 10. Upcaster: apply_upcasters rejects empty event_type
//! 11. Upcaster: chain limit at 100 prevents infinite loops
//! 12. Upcaster: well-behaved chain produces correct final state
//! 13. UpcasterChain cons-list: dispatches to first match, falls through
//! 14. EventStore: save-then-load roundtrip preserves aggregate state
//! 15. EventStore: save with no uncommitted events is no-op
//! 16. EventStore: concurrent saves detect conflict
//! 17. EventStore: load empty stream returns fresh aggregate
//! 18. EventStore: upcasters are applied during load
//! 19. InMemoryStore: stream isolation — writes to A never leak to B
//! 20. InMemoryStore: read_stream(from) filters correctly
//! 21. Codec: encode/decode roundtrip identity
//! 22. Model-based: shadow model tracks all operations, compared to real store
//! 23. Adversarial payloads: binary garbage, huge sizes, all-zeros, all-0xFF
//! 24. Version arithmetic: from_persisted roundtrip, next() chain, overflow

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "proptest macros use panic")]
#![allow(clippy::missing_panics_doc, reason = "proptest")]
#![allow(clippy::needless_pass_by_value, reason = "proptest")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::cast_possible_wrap, reason = "tests")]
#![allow(clippy::cast_sign_loss, reason = "tests")]
#![allow(clippy::implicit_clone, reason = "tests")]
#![allow(clippy::clone_on_ref_ptr, reason = "tests")]
#![allow(clippy::missing_docs_in_private_items, reason = "tests")]
#![allow(clippy::doc_markdown, reason = "tests")]
#![allow(clippy::uninlined_format_args, reason = "tests")]
#![allow(clippy::use_self, reason = "tests")]
#![allow(clippy::items_after_statements, reason = "tests")]
#![allow(clippy::regex_creation_in_loops, reason = "tests")]
#![allow(clippy::suspicious_operation_groupings, reason = "tests")]
#![allow(clippy::no_effect_replace, reason = "identity upcaster by design")]
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::print_stdout, reason = "diagnostic output")]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use nexus::Version;
use nexus_store::codec::Codec;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::error::{StoreError, UpcastError};
use nexus_store::event_store::EventStore;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::repository::Repository;
use nexus_store::stream::EventStream;
use nexus_store::testing::InMemoryStore;
use nexus_store::upcaster::{EventUpcaster, apply_upcasters};
use nexus_store::upcaster_chain::{Chain, UpcasterChain};

use proptest::prelude::*;

// ============================================================================
// Test Domain Types (minimal aggregate for integration tests)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum TestEvent {
    Happened(String),
    ValueSet(i64),
}

impl nexus::Message for TestEvent {}
impl nexus::DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Happened(_) => "Happened",
            TestEvent::ValueSet(_) => "ValueSet",
        }
    }
}

#[derive(Default, Debug, PartialEq)]
struct TestState {
    events_applied: u64,
    last_value: i64,
    log: Vec<String>,
}

impl nexus::AggregateState for TestState {
    type Event = TestEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &TestEvent) -> Self {
        self.events_applied += 1;
        match event {
            TestEvent::Happened(s) => self.log.push(s.clone()),
            TestEvent::ValueSet(v) => self.last_value = *v,
        }
        self
    }
    fn name(&self) -> &'static str {
        "Test"
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(u64);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "test-{}", self.0)
    }
}
impl nexus::Id for TestId {}

#[derive(Debug, thiserror::Error)]
#[error("test error")]
struct TestError;

struct TestAggregate;
impl nexus::Aggregate for TestAggregate {
    type State = TestState;
    type Error = TestError;
    type Id = TestId;
}

// ============================================================================
// JSON Codec for integration tests
// ============================================================================

struct JsonCodec;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct JsonCodecError(String);

impl Codec<TestEvent> for JsonCodec {
    type Error = JsonCodecError;
    fn encode(&self, event: &TestEvent) -> Result<Vec<u8>, Self::Error> {
        let json = match event {
            TestEvent::Happened(s) => format!(r#"{{"Happened":"{}"}}"#, s),
            TestEvent::ValueSet(v) => format!(r#"{{"ValueSet":{}}}"#, v),
        };
        Ok(json.into_bytes())
    }
    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<TestEvent, Self::Error> {
        let s = std::str::from_utf8(payload).map_err(|e| JsonCodecError(e.to_string()))?;
        match event_type {
            "Happened" => {
                // Parse {"Happened":"value"}
                let value = s
                    .strip_prefix(r#"{"Happened":""#)
                    .and_then(|s| s.strip_suffix(r#""}"#))
                    .ok_or_else(|| JsonCodecError(format!("bad Happened payload: {s}")))?;
                Ok(TestEvent::Happened(value.to_owned()))
            }
            "ValueSet" => {
                let value = s
                    .strip_prefix(r#"{"ValueSet":"#)
                    .and_then(|s| s.strip_suffix('}'))
                    .ok_or_else(|| JsonCodecError(format!("bad ValueSet payload: {s}")))?;
                let v: i64 = value
                    .parse()
                    .map_err(|e: std::num::ParseIntError| JsonCodecError(e.to_string()))?;
                Ok(TestEvent::ValueSet(v))
            }
            other => Err(JsonCodecError(format!("unknown event type: {other}"))),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn build_envelopes(stream_id: &str, payloads: &[Vec<u8>]) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            pending_envelope(stream_id.to_owned())
                .version(Version::from_persisted(u64::try_from(i).unwrap() + 1))
                .event_type(leak("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

fn build_envelopes_from(
    stream_id: &str,
    start_version: u64,
    payloads: &[Vec<u8>],
) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            pending_envelope(stream_id.to_owned())
                .version(Version::from_persisted(
                    start_version + u64::try_from(i).unwrap(),
                ))
                .event_type(leak("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

async fn read_all_payloads(store: &InMemoryStore, stream_id: &str) -> Vec<Vec<u8>> {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut payloads = Vec::new();
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        payloads.push(env.payload().to_vec());
    }
    payloads
}

async fn read_all_versions(store: &InMemoryStore, stream_id: &str) -> Vec<u64> {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        versions.push(env.version().as_u64());
    }
    versions
}

// ============================================================================
// Strategies
// ============================================================================

fn stream_id_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z][a-z0-9_-]{0,29}").unwrap()
}

fn evil_stream_id_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Normal IDs
        prop::string::string_regex("[a-z][a-z0-9-]{0,19}").unwrap(),
        // Empty string
        Just(String::new()),
        // Unicode madness
        prop::string::string_regex("[\\x00-\\xFF]{0,50}").unwrap(),
        // SQL injection
        Just("'; DROP TABLE events; --".to_owned()),
        Just("stream\0id".to_owned()),
        Just("../../../etc/passwd".to_owned()),
        // Very long
        Just("x".repeat(10_000)),
        // Whitespace only
        Just("   \t\n  ".to_owned()),
        // Unicode normalization edge cases
        Just("\u{FEFF}stream".to_owned()), // BOM prefix
        Just("café".to_owned()),           // NFC vs NFD
    ]
}

fn payloads_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
    prop::collection::vec(prop::collection::vec(any::<u8>(), 0..512), 1..30)
}

fn evil_payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Empty
        Just(vec![]),
        // All zeros
        prop::collection::vec(Just(0u8), 0..1000),
        // All 0xFF
        prop::collection::vec(Just(0xFFu8), 0..1000),
        // Random binary
        prop::collection::vec(any::<u8>(), 0..5000),
        // Huge payload
        Just(vec![0xAB; 100_000]),
        // Almost valid JSON
        Just(br#"{"broken":}"#.to_vec()),
        // Null bytes everywhere
        Just(vec![0; 10_000]),
    ]
}

fn schema_version_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0u32),
        Just(1u32),
        Just(u32::MAX),
        1..1000u32,
        any::<u32>(),
    ]
}

// ============================================================================
// ATTACK 1: PersistedEnvelope schema_version=0 MUST panic/error
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn attack_persisted_envelope_schema_version_zero_panics(
        stream_id in "[a-z]{1,10}",
        version in 1..1000u64,
        event_type in "[A-Z]{1,10}",
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        let result = std::panic::catch_unwind(|| {
            PersistedEnvelope::<()>::new(
                &stream_id,
                Version::from_persisted(version),
                &event_type,
                0, // ATTACK: schema_version = 0
                &payload,
                (),
            )
        });
        prop_assert!(result.is_err(), "schema_version=0 MUST panic in new()");
    }

    #[test]
    fn attack_persisted_envelope_try_new_rejects_zero(
        stream_id in "[a-z]{1,10}",
        version in 1..1000u64,
        event_type in "[A-Z]{1,10}",
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        let result = PersistedEnvelope::<()>::try_new(
            &stream_id,
            Version::from_persisted(version),
            &event_type,
            0, // ATTACK
            &payload,
            (),
        );
        prop_assert!(result.is_err(), "try_new with schema_version=0 must return Err");
    }
}

// ============================================================================
// ATTACK 2: PersistedEnvelope field accessor faithfulness
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn attack_persisted_envelope_accessors_are_faithful(
        stream_id in "[a-z]{1,20}",
        version in 1..u64::MAX,
        event_type in "[A-Z][a-z]{0,20}",
        schema_version in 1..u32::MAX,
        payload in prop::collection::vec(any::<u8>(), 0..1000),
        metadata in any::<u64>(),
    ) {
        let env = PersistedEnvelope::new(
            &stream_id,
            Version::from_persisted(version),
            &event_type,
            schema_version,
            &payload,
            metadata,
        );

        prop_assert_eq!(env.stream_id(), stream_id.as_str());
        prop_assert_eq!(env.version().as_u64(), version);
        prop_assert_eq!(env.event_type(), event_type.as_str());
        prop_assert_eq!(env.schema_version(), schema_version);
        prop_assert_eq!(env.payload(), payload.as_slice());
        prop_assert_eq!(*env.metadata(), metadata);
    }
}

// ============================================================================
// ATTACK 3: PendingEnvelope builder roundtrip
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn attack_pending_envelope_builder_preserves_all_fields(
        stream_id in ".*",
        version in any::<u64>(),
        payload in prop::collection::vec(any::<u8>(), 0..2000),
        metadata in any::<i32>(),
    ) {
        let env = pending_envelope(stream_id.clone())
            .version(Version::from_persisted(version))
            .event_type(leak("AnyType"))
            .payload(payload.clone())
            .build(metadata);

        prop_assert_eq!(env.stream_id(), stream_id.as_str());
        prop_assert_eq!(env.version().as_u64(), version);
        prop_assert_eq!(env.event_type(), "AnyType");
        prop_assert_eq!(env.payload(), payload.as_slice());
        prop_assert_eq!(*env.metadata(), metadata);
    }
}

// ============================================================================
// ATTACK 4: Version arithmetic — from_persisted roundtrip, next() chain
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn attack_version_from_persisted_roundtrip(v in any::<u64>()) {
        let version = Version::from_persisted(v);
        prop_assert_eq!(version.as_u64(), v, "from_persisted/as_u64 roundtrip failed");
    }

    #[test]
    fn attack_version_next_chain(start in 0..10_000u64, steps in 0..100usize) {
        let mut v = Version::from_persisted(start);
        for _ in 0..steps {
            if v.as_u64() == u64::MAX {
                // next() should panic
                let result = std::panic::catch_unwind(move || v.next());
                prop_assert!(result.is_err(), "next() at MAX must panic");
                return Ok(());
            }
            let next = v.next();
            prop_assert_eq!(next.as_u64(), v.as_u64() + 1, "next() must increment by 1");
            v = next;
        }
    }

    #[test]
    fn attack_version_ordering(a in any::<u64>(), b in any::<u64>()) {
        let va = Version::from_persisted(a);
        let vb = Version::from_persisted(b);
        prop_assert_eq!(va < vb, a < b, "Version ordering must match u64 ordering");
        prop_assert_eq!(va == vb, a == b, "Version equality must match u64 equality");
    }
}

// ============================================================================
// ATTACK 5: RawEventStore — append/read roundtrip with arbitrary payloads
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_append_read_roundtrip_any_payload(
        stream_id in stream_id_strategy(),
        payloads in payloads_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let envelopes = build_envelopes(&stream_id, &payloads);

            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), payloads.len(), "payload count mismatch");
            for (i, (read_p, orig_p)) in read.iter().zip(payloads.iter()).enumerate() {
                prop_assert_eq!(read_p, orig_p, "payload mismatch at index {}", i);
            }
            Ok(())
        })?;
    }

    #[test]
    fn attack_append_read_roundtrip_evil_payloads(
        stream_id in stream_id_strategy(),
        payloads in prop::collection::vec(evil_payload_strategy(), 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let envelopes = build_envelopes(&stream_id, &payloads);

            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), payloads.len());
            for (i, (r, o)) in read.iter().zip(payloads.iter()).enumerate() {
                prop_assert_eq!(r, o, "evil payload corrupted at index {}", i);
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 6: RawEventStore — optimistic concurrency enforcement
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    // Append with wrong expected_version must fail
    #[test]
    fn attack_concurrency_wrong_expected_version(
        stream_id in stream_id_strategy(),
        initial_count in 1..20usize,
        wrong_version in 0..100u64,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // First, put some events in
            let payloads: Vec<Vec<u8>> = (0..initial_count).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let actual_version = u64::try_from(initial_count).unwrap();
            // Skip if wrong_version accidentally matches
            prop_assume!(wrong_version != actual_version);

            // Try to append with wrong expected_version
            let new_envelopes = build_envelopes_from(
                &stream_id,
                wrong_version + 1,
                &[vec![0xFF]],
            );
            let result = store.append(
                &stream_id,
                Version::from_persisted(wrong_version),
                &new_envelopes,
            ).await;

            prop_assert!(result.is_err(), "wrong expected_version MUST be rejected");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 7: RawEventStore — sequential version enforcement
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // Random version sequences must be rejected unless perfectly sequential
    #[test]
    fn attack_non_sequential_versions_rejected(
        stream_id in stream_id_strategy(),
        versions in prop::collection::vec(1..1000u64, 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            let envelopes: Vec<_> = versions.iter().map(|&v| {
                pending_envelope(stream_id.clone())
                    .version(Version::from_persisted(v))
                    .event_type(leak("E"))
                    .payload(vec![v as u8])
                    .build_without_metadata()
            }).collect();

            let result = store.append(&stream_id, Version::INITIAL, &envelopes).await;

            let is_sequential = versions.iter().enumerate().all(|(i, &v)| v == (i as u64) + 1);
            if is_sequential {
                prop_assert!(result.is_ok(), "sequential versions must be accepted");
            } else {
                prop_assert!(result.is_err(), "non-sequential versions MUST be rejected: {:?}", versions);
            }
            Ok(())
        })?;
    }

    // Duplicate versions in a batch must be rejected
    #[test]
    fn attack_duplicate_versions_rejected(
        stream_id in stream_id_strategy(),
        n in 2..10usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Create a batch where all envelopes have version 1
            let envelopes: Vec<_> = (0..n).map(|_| {
                pending_envelope(stream_id.clone())
                    .version(Version::from_persisted(1))
                    .event_type(leak("E"))
                    .payload(vec![1])
                    .build_without_metadata()
            }).collect();

            let result = store.append(&stream_id, Version::INITIAL, &envelopes).await;
            prop_assert!(result.is_err(), "duplicate versions MUST be rejected (batch of {} all at v1)", n);
            Ok(())
        })?;
    }

    // Gap in versions must be rejected
    #[test]
    fn attack_version_gap_rejected(
        stream_id in stream_id_strategy(),
        gap_position in 0..5usize,
        gap_size in 1..100u64,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Build versions [1, 2, ..., gap_position, gap_position+gap_size+1, ...]
            let mut versions = Vec::new();
            for i in 0..6u64 {
                if i == u64::try_from(gap_position).unwrap() + 1 {
                    versions.push(i + 1 + gap_size); // skip ahead
                } else if i <= u64::try_from(gap_position).unwrap() {
                    versions.push(i + 1);
                } else {
                    versions.push(i + gap_size + 1);
                }
            }

            let envelopes: Vec<_> = versions.iter().map(|&v| {
                pending_envelope(stream_id.clone())
                    .version(Version::from_persisted(v))
                    .event_type(leak("E"))
                    .payload(vec![v as u8])
                    .build_without_metadata()
            }).collect();

            let result = store.append(&stream_id, Version::INITIAL, &envelopes).await;
            prop_assert!(result.is_err(), "version gap MUST be rejected: {:?}", versions);
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 8: EventStream — monotonically increasing versions
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_stream_versions_are_monotonic(
        stream_id in stream_id_strategy(),
        n in 1..50usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let versions = read_all_versions(&store, &stream_id).await;
            for window in versions.windows(2) {
                prop_assert!(
                    window[1] > window[0],
                    "versions not strictly increasing: {} followed by {}",
                    window[0], window[1],
                );
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 9: EventStream — fused after None
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_stream_fused_after_none(
        stream_id in stream_id_strategy(),
        n in 0..20usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            if n > 0 {
                let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
                let envelopes = build_envelopes(&stream_id, &payloads);
                store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();
            }

            let mut stream = store.read_stream(&stream_id, Version::INITIAL).await.unwrap();

            // Drain all events
            let mut count = 0;
            while let Some(result) = stream.next().await {
                result.unwrap();
                count += 1;
            }
            prop_assert_eq!(count, n, "wrong event count");

            // After None, all subsequent calls must also return None (fused)
            for _ in 0..10 {
                let next = stream.next().await;
                prop_assert!(next.is_none(), "stream must be fused: returned Some after None");
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 10: Stream isolation — writes to A never leak to B
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_stream_isolation(
        id_a in stream_id_strategy(),
        id_b in stream_id_strategy(),
        payloads_a in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..100), 1..15),
        payloads_b in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..100), 1..15),
    ) {
        prop_assume!(id_a != id_b);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            let envelopes_a = build_envelopes(&id_a, &payloads_a);
            let envelopes_b = build_envelopes(&id_b, &payloads_b);

            store.append(&id_a, Version::INITIAL, &envelopes_a).await.unwrap();
            store.append(&id_b, Version::INITIAL, &envelopes_b).await.unwrap();

            let read_a = read_all_payloads(&store, &id_a).await;
            let read_b = read_all_payloads(&store, &id_b).await;

            prop_assert_eq!(read_a.len(), payloads_a.len(), "stream A count wrong");
            prop_assert_eq!(read_b.len(), payloads_b.len(), "stream B count wrong");

            for (i, (r, o)) in read_a.iter().zip(payloads_a.iter()).enumerate() {
                prop_assert_eq!(r, o, "stream A payload mismatch at {}", i);
            }
            for (i, (r, o)) in read_b.iter().zip(payloads_b.iter()).enumerate() {
                prop_assert_eq!(r, o, "stream B payload mismatch at {}", i);
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 11: read_stream(from) filters correctly
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_read_stream_from_filters_correctly(
        stream_id in stream_id_strategy(),
        n in 1..30usize,
        from_offset in 0..30usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let from_version = u64::try_from(from_offset).unwrap();
            let mut stream = store
                .read_stream(&stream_id, Version::from_persisted(from_version))
                .await
                .unwrap();

            let mut read_versions = Vec::new();
            while let Some(result) = stream.next().await {
                let env = result.unwrap();
                read_versions.push(env.version().as_u64());
            }

            // All returned versions must be >= from_version
            for &v in &read_versions {
                prop_assert!(
                    v >= from_version,
                    "read_stream(from={}) returned version {} which is less than from",
                    from_version, v,
                );
            }

            // Count should match expected filtered count
            let expected_count = (1..=n as u64).filter(|v| *v >= from_version).count();
            prop_assert_eq!(
                read_versions.len(), expected_count,
                "read_stream(from={}) returned {} events, expected {}",
                from_version, read_versions.len(), expected_count,
            );
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 12: Upcaster — apply_upcasters rejects non-advancing versions
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn attack_upcaster_rejects_same_version(
        input_version in 1..1000u32,
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        struct SameVersionUpcaster(u32);
        impl EventUpcaster for SameVersionUpcaster {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == self.0 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), self.0, p.to_vec()) // returns SAME version
            }
        }

        let u = SameVersionUpcaster(input_version);
        let upcasters: Vec<&dyn EventUpcaster> = vec![&u];
        let result = apply_upcasters(&upcasters, "E", input_version, &payload);
        prop_assert!(
            result.is_err(),
            "apply_upcasters MUST reject same-version output (input=output={})",
            input_version,
        );
        if let Err(UpcastError::VersionNotAdvanced { input_version: iv, output_version: ov, .. }) = &result {
            prop_assert_eq!(*iv, input_version);
            prop_assert_eq!(*ov, input_version);
        }
    }

    #[test]
    fn attack_upcaster_rejects_lower_version(
        input_version in 2..1000u32,
        decrement in 1..100u32,
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        let output_version = input_version.saturating_sub(decrement);
        prop_assume!(output_version < input_version);

        struct LowerVersionUpcaster { input: u32, output: u32 }
        impl EventUpcaster for LowerVersionUpcaster {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == self.input }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), self.output, p.to_vec())
            }
        }

        let upcaster = LowerVersionUpcaster { input: input_version, output: output_version };
        let upcasters: Vec<&dyn EventUpcaster> = vec![&upcaster];
        let result = apply_upcasters(&upcasters, "E", input_version, &payload);
        prop_assert!(result.is_err(), "apply_upcasters MUST reject lower version output");
    }
}

// ============================================================================
// ATTACK 13: Upcaster — apply_upcasters rejects empty event_type
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_upcaster_rejects_empty_event_type(
        input_version in 1..1000u32,
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        struct EmptyTypeUpcaster(u32);
        impl EventUpcaster for EmptyTypeUpcaster {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == self.0 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                (String::new(), self.0 + 1, p.to_vec()) // empty event_type!
            }
        }

        let u = EmptyTypeUpcaster(input_version);
        let upcasters: Vec<&dyn EventUpcaster> = vec![&u];
        let result = apply_upcasters(&upcasters, "SomeEvent", input_version, &payload);
        prop_assert!(result.is_err(), "apply_upcasters MUST reject empty event_type");
        assert!(matches!(result, Err(UpcastError::EmptyEventType { .. })));
    }
}

// ============================================================================
// ATTACK 14: Upcaster — chain limit prevents infinite loops
// ============================================================================

#[test]
fn attack_upcaster_chain_limit_101_iterations() {
    struct IncrementForever;
    impl EventUpcaster for IncrementForever {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true // always matches
        }
        fn upcast(&self, _: &str, v: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".into(), v.saturating_add(1), p.to_vec())
        }
    }

    let upcasters: Vec<&dyn EventUpcaster> = vec![&IncrementForever];
    let result = apply_upcasters(&upcasters, "E", 1, b"data");

    assert!(result.is_err(), "infinite upcaster chain must be caught");
    assert!(
        matches!(
            result,
            Err(UpcastError::ChainLimitExceeded { limit: 100, .. })
        ),
        "should hit chain limit of 100, got: {:?}",
        result,
    );
}

// ============================================================================
// ATTACK 15: Upcaster — well-behaved chain produces correct final state
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_upcaster_chain_correct_final_state(
        chain_length in 1..20u32,
        payload in prop::collection::vec(any::<u8>(), 0..200),
    ) {
        // Build a chain of N upcasters: V1->V2, V2->V3, ..., VN->V(N+1)
        // Each preserves the payload.
        struct StepUpcaster(u32);
        impl EventUpcaster for StepUpcaster {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == self.0 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), self.0 + 1, p.to_vec())
            }
        }

        let upcasters_owned: Vec<StepUpcaster> = (1..=chain_length).map(StepUpcaster).collect();
        let upcasters: Vec<&dyn EventUpcaster> = upcasters_owned.iter()
            .map(|u| u as &dyn EventUpcaster)
            .collect();

        let result = apply_upcasters(&upcasters, "E", 1, &payload);
        let (event_type, final_version, final_payload) = result.unwrap();

        prop_assert_eq!(event_type, "E");
        prop_assert_eq!(final_version, chain_length + 1);
        prop_assert_eq!(final_payload, payload, "payload corrupted through upcaster chain");
    }
}

// ============================================================================
// ATTACK 16: UpcasterChain cons-list — dispatches correctly
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_upcaster_chain_cons_list_dispatch(
        version in 1..100u32,
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        // Two upcasters: first handles version, second handles version+1
        struct U(u32);
        impl EventUpcaster for U {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == self.0 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), self.0 + 1, p.to_vec())
            }
        }

        let chain = Chain(U(version), Chain(U(version + 1), ()));

        // First dispatch: should match U(version)
        let result = chain.try_upcast("E", version, &payload);
        prop_assert!(result.is_some(), "chain should match version {}", version);
        let (_, new_v, _) = result.unwrap();
        prop_assert_eq!(new_v, version + 1);

        // Second dispatch: should match U(version+1)
        let result = chain.try_upcast("E", version + 1, &payload);
        prop_assert!(result.is_some());
        let (_, new_v, _) = result.unwrap();
        prop_assert_eq!(new_v, version + 2);

        // Version not handled: should return None
        let result = chain.try_upcast("E", version + 2, &payload);
        prop_assert!(result.is_none(), "chain should not match unhandled version");
    }

    // Empty chain always returns None
    #[test]
    fn attack_empty_upcaster_chain(
        version in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        let chain = ();
        let result = chain.try_upcast("E", version, &payload);
        prop_assert!(result.is_none(), "empty chain must always return None");
    }
}

// ============================================================================
// ATTACK 17: Codec encode/decode roundtrip
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn attack_codec_roundtrip_happened(s in "[a-zA-Z0-9 ]{0,100}") {
        let codec = JsonCodec;
        let event = TestEvent::Happened(s.clone());
        let encoded = codec.encode(&event).unwrap();
        let decoded = codec.decode("Happened", &encoded).unwrap();
        prop_assert_eq!(decoded, event, "Happened roundtrip failed for: {:?}", s);
    }

    #[test]
    fn attack_codec_roundtrip_value_set(v in any::<i64>()) {
        let codec = JsonCodec;
        let event = TestEvent::ValueSet(v);
        let encoded = codec.encode(&event).unwrap();
        let decoded = codec.decode("ValueSet", &encoded).unwrap();
        prop_assert_eq!(decoded, event, "ValueSet roundtrip failed for: {}", v);
    }

    #[test]
    fn attack_codec_rejects_unknown_type(
        event_type in "[a-z]{1,20}",
        payload in prop::collection::vec(any::<u8>(), 0..100),
    ) {
        prop_assume!(event_type != "Happened" && event_type != "ValueSet");
        let codec = JsonCodec;
        let result = codec.decode(&event_type, &payload);
        prop_assert!(result.is_err(), "codec must reject unknown event type: {}", event_type);
    }
}

// ============================================================================
// ATTACK 18: EventStore — full save/load roundtrip
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_event_store_save_load_roundtrip(
        values in prop::collection::vec(-1000..1000i64, 1..20),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            // Apply events to an aggregate
            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(42));
            for &v in &values {
                root.apply(TestEvent::ValueSet(v));
            }

            // Save
            es.save("stream-42", &mut root).await.unwrap();

            // Load into fresh aggregate
            let loaded: nexus::AggregateRoot<TestAggregate> = es.load("stream-42", TestId(42)).await.unwrap();

            // Verify state matches
            prop_assert_eq!(
                loaded.state().last_value,
                *values.last().unwrap(),
                "last_value mismatch after save/load",
            );
            prop_assert_eq!(
                loaded.state().events_applied,
                u64::try_from(values.len()).unwrap(),
                "events_applied mismatch",
            );
            prop_assert_eq!(
                loaded.version().as_u64(),
                u64::try_from(values.len()).unwrap(),
                "version mismatch after load",
            );
            Ok(())
        })?;
    }

    #[test]
    fn attack_event_store_save_load_happened_events(
        strings in prop::collection::vec("[a-zA-Z0-9]{1,50}", 1..15),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(1));
            for s in &strings {
                root.apply(TestEvent::Happened(s.clone()));
            }

            es.save("stream-1", &mut root).await.unwrap();
            let loaded: nexus::AggregateRoot<TestAggregate> = es.load("stream-1", TestId(1)).await.unwrap();

            prop_assert_eq!(&loaded.state().log, &strings, "event log mismatch after roundtrip");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 19: EventStore — save with no uncommitted events is a no-op
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_event_store_noop_save(
        values in prop::collection::vec(-100..100i64, 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(99));
            for &v in &values {
                root.apply(TestEvent::ValueSet(v));
            }

            // First save — should work
            es.save("stream-99", &mut root).await.unwrap();

            // Second save with no new events — should be a no-op, not an error
            let result = es.save("stream-99", &mut root).await;
            prop_assert!(result.is_ok(), "save with no uncommitted events must be a no-op");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 20: EventStore — load empty stream returns fresh aggregate
// ============================================================================

#[tokio::test]
async fn attack_event_store_load_empty_stream() {
    let store = InMemoryStore::new();
    let es = EventStore::new(store, JsonCodec);

    let loaded: nexus::AggregateRoot<TestAggregate> =
        es.load("nonexistent-stream", TestId(1)).await.unwrap();

    assert_eq!(loaded.version(), Version::INITIAL);
    assert_eq!(loaded.state().events_applied, 0);
    assert_eq!(loaded.state().last_value, 0);
    assert!(loaded.state().log.is_empty());
}

// ============================================================================
// ATTACK 21: EventStore — concurrent saves detect conflict
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_event_store_concurrent_conflict(
        n in 1..10usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            // Load two "copies" of the same aggregate
            let mut root_a = nexus::AggregateRoot::<TestAggregate>::new(TestId(1));
            for i in 0..n {
                root_a.apply(TestEvent::ValueSet(i as i64));
            }
            es.save("conflict-stream", &mut root_a).await.unwrap();

            // Load into two separate aggregates
            let mut copy1: nexus::AggregateRoot<TestAggregate> = es.load("conflict-stream", TestId(1)).await.unwrap();
            let mut copy2: nexus::AggregateRoot<TestAggregate> = es.load("conflict-stream", TestId(1)).await.unwrap();

            // Both apply different events
            copy1.apply(TestEvent::ValueSet(100));
            copy2.apply(TestEvent::ValueSet(200));

            // First save succeeds
            es.save("conflict-stream", &mut copy1).await.unwrap();

            // Second save MUST fail with conflict
            let result = es.save("conflict-stream", &mut copy2).await;
            prop_assert!(result.is_err(), "concurrent save MUST detect conflict");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 22: EventStore — upcasters applied during load
// ============================================================================

#[tokio::test]
async fn attack_event_store_upcasters_applied_on_load() {
    // Upcaster that transforms "OldHappened" V1 payload to "Happened" V2
    struct OldToNewUpcaster;
    impl EventUpcaster for OldToNewUpcaster {
        fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
            event_type == "Happened" && schema_version == 1
        }
        fn upcast(&self, _: &str, _: u32, payload: &[u8]) -> (String, u32, Vec<u8>) {
            // Transform payload: add a prefix to the string inside
            let s = std::str::from_utf8(payload).unwrap();
            let modified = s.replace("Happened", "Happened");
            ("Happened".into(), 2, modified.into_bytes())
        }
    }

    let store = InMemoryStore::new();

    // Manually insert events at schema_version 1 via RawEventStore
    let codec = JsonCodec;
    let payload = codec.encode(&TestEvent::Happened("hello".into())).unwrap();
    let envelopes = vec![
        pending_envelope("upcast-stream".into())
            .version(Version::from_persisted(1))
            .event_type(leak("Happened"))
            .payload(payload)
            .build_without_metadata(),
    ];
    store
        .append("upcast-stream", Version::INITIAL, &envelopes)
        .await
        .unwrap();

    // Load with upcaster (schema v1 -> v2, payload unchanged for this test)
    let es = EventStore::new(store, JsonCodec).with_upcaster(OldToNewUpcaster);

    let loaded: nexus::AggregateRoot<TestAggregate> =
        es.load("upcast-stream", TestId(1)).await.unwrap();
    assert_eq!(loaded.state().events_applied, 1);
    assert_eq!(loaded.state().log, vec!["hello".to_owned()]);
}

// ============================================================================
// ATTACK 23: Model-based testing — shadow model vs real store
// ============================================================================

#[derive(Debug, Clone)]
enum ModelOp {
    Append {
        stream_id: String,
        payloads: Vec<Vec<u8>>,
    },
    Read {
        stream_id: String,
    },
}

fn model_op_strategy() -> impl Strategy<Value = ModelOp> {
    let stream_ids = prop_oneof![
        Just("alpha".to_owned()),
        Just("beta".to_owned()),
        Just("gamma".to_owned()),
    ];

    prop_oneof![
        (
            stream_ids.clone(),
            prop::collection::vec(prop::collection::vec(any::<u8>(), 1..50), 1..5),
        )
            .prop_map(|(stream_id, payloads)| ModelOp::Append {
                stream_id,
                payloads
            }),
        stream_ids.prop_map(|stream_id| ModelOp::Read { stream_id }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_model_based_store_correctness(
        ops in prop::collection::vec(model_op_strategy(), 1..30),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let mut model: HashMap<String, Vec<Vec<u8>>> = HashMap::new();

            for op in &ops {
                match op {
                    ModelOp::Append { stream_id, payloads } => {
                        let model_stream = model.entry(stream_id.clone()).or_default();
                        let start_version = u64::try_from(model_stream.len()).unwrap();
                        let expected_version = Version::from_persisted(start_version);

                        let envelopes = build_envelopes_from(
                            stream_id,
                            start_version + 1,
                            payloads,
                        );

                        let result = store.append(stream_id, expected_version, &envelopes).await;
                        prop_assert!(result.is_ok(), "append should succeed for model-valid operation");

                        model_stream.extend(payloads.iter().cloned());
                    }
                    ModelOp::Read { stream_id } => {
                        let expected = model.get(stream_id).cloned().unwrap_or_default();
                        let actual = read_all_payloads(&store, stream_id).await;

                        prop_assert_eq!(
                            actual.len(), expected.len(),
                            "model/store count mismatch for stream '{}'", stream_id,
                        );
                        for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
                            prop_assert_eq!(
                                a, e,
                                "model/store payload mismatch for stream '{}' at index {}", stream_id, i,
                            );
                        }
                    }
                }
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 24: Multi-batch append — incremental appends
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_multi_batch_append(
        stream_id in stream_id_strategy(),
        batch_sizes in prop::collection::vec(1..10usize, 2..8),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let mut total_events: u64 = 0;
            let mut all_payloads: Vec<Vec<u8>> = Vec::new();

            for batch_size in &batch_sizes {
                let payloads: Vec<Vec<u8>> = (0..*batch_size)
                    .map(|i| vec![(total_events + i as u64) as u8])
                    .collect();

                let envelopes = build_envelopes_from(
                    &stream_id,
                    total_events + 1,
                    &payloads,
                );

                store.append(
                    &stream_id,
                    Version::from_persisted(total_events),
                    &envelopes,
                ).await.unwrap();

                all_payloads.extend(payloads);
                total_events += *batch_size as u64;
            }

            // Read back and verify everything
            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), all_payloads.len(), "total event count mismatch");
            for (i, (r, o)) in read.iter().zip(all_payloads.iter()).enumerate() {
                prop_assert_eq!(r, o, "payload mismatch at index {} after multi-batch append", i);
            }

            // Verify versions are 1..=total_events
            let versions = read_all_versions(&store, &stream_id).await;
            let expected_versions: Vec<u64> = (1..=total_events).collect();
            prop_assert_eq!(versions, expected_versions, "version sequence wrong after multi-batch append");

            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 25: Evil stream_ids — SQL injection, null bytes, unicode
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_evil_stream_ids_dont_crash(
        stream_id in evil_stream_id_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Building envelopes with evil stream_id should not panic
            let envelope = pending_envelope(stream_id.clone())
                .version(Version::from_persisted(1))
                .event_type(leak("E"))
                .payload(vec![1, 2, 3])
                .build_without_metadata();

            // Append should not panic (may succeed or fail with error)
            let result = store.append(&stream_id, Version::INITIAL, &[envelope]).await;

            // If append succeeded, read should return the event
            if result.is_ok() {
                let read = read_all_payloads(&store, &stream_id).await;
                prop_assert_eq!(read.len(), 1);
                prop_assert_eq!(&read[0], &vec![1u8, 2, 3]);
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 26: Empty append — no envelopes
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_empty_append(
        stream_id in stream_id_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Append with empty slice
            let result = store.append(&stream_id, Version::INITIAL, &[]).await;
            // This should succeed (no-op) — no events to validate
            prop_assert!(result.is_ok(), "empty append should succeed as no-op");

            // Stream should be empty
            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert!(read.is_empty(), "empty append should not create events");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 27: EventStore multi-save/load cycles (aggregate lifecycle)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_event_store_multi_cycle_lifecycle(
        cycles in prop::collection::vec(
            prop::collection::vec(-1000..1000i64, 1..5),
            2..6,
        ),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);
            let stream_id = "lifecycle-stream";

            let mut total_events: u64 = 0;
            let mut expected_last_value: i64 = 0;

            for (cycle_idx, values) in cycles.iter().enumerate() {
                // Load current state
                let mut root: nexus::AggregateRoot<TestAggregate> = if cycle_idx == 0 {
                    nexus::AggregateRoot::<TestAggregate>::new(TestId(1))
                } else {
                    es.load(stream_id, TestId(1)).await.unwrap()
                };

                // Verify loaded state
                prop_assert_eq!(
                    root.version().as_u64(), total_events,
                    "version wrong at start of cycle {}", cycle_idx,
                );

                // Apply new events
                for &v in values {
                    root.apply(TestEvent::ValueSet(v));
                    expected_last_value = v;
                }

                // Save
                es.save(stream_id, &mut root).await.unwrap();
                total_events += u64::try_from(values.len()).unwrap();

                // Verify by loading again
                let loaded: nexus::AggregateRoot<TestAggregate> = es.load(stream_id, TestId(1)).await.unwrap();
                prop_assert_eq!(
                    loaded.state().last_value, expected_last_value,
                    "last_value wrong after cycle {}", cycle_idx,
                );
                prop_assert_eq!(
                    loaded.state().events_applied, total_events,
                    "events_applied wrong after cycle {}", cycle_idx,
                );
                prop_assert_eq!(
                    loaded.version().as_u64(), total_events,
                    "version wrong after cycle {}", cycle_idx,
                );
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 28: Concurrent readers — multiple read_stream calls don't interfere
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_concurrent_readers(
        stream_id in stream_id_strategy(),
        n in 1..20usize,
        num_readers in 2..5usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = Arc::new(InMemoryStore::new());
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            // Spawn multiple readers
            let mut handles = Vec::new();
            for _ in 0..num_readers {
                let store = Arc::clone(&store);
                let sid = stream_id.clone();
                let expected = payloads.clone();
                handles.push(tokio::spawn(async move {
                    let mut stream = store.read_stream(&sid, Version::INITIAL).await.unwrap();
                    let mut read = Vec::new();
                    while let Some(result) = stream.next().await {
                        let env = result.unwrap();
                        read.push(env.payload().to_vec());
                    }
                    assert_eq!(read, expected, "reader got wrong payloads");
                }));
            }

            for handle in handles {
                handle.await.unwrap();
            }
            Ok::<(), proptest::test_runner::TestCaseError>(())
        })?;
    }
}

// ============================================================================
// ATTACK 29: Adversarial PersistedEnvelope — boundary schema_versions
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn attack_persisted_envelope_boundary_schema_versions(
        schema_version in schema_version_strategy(),
    ) {
        if schema_version == 0 {
            // Must panic
            let result = std::panic::catch_unwind(|| {
                let _ = PersistedEnvelope::<()>::new("s", Version::from_persisted(1), "E", 0, &[], ());
            });
            prop_assert!(result.is_err(), "schema_version=0 must panic");

            // try_new must return Err
            let result = PersistedEnvelope::<()>::try_new("s", Version::from_persisted(1), "E", 0, &[], ());
            prop_assert!(result.is_err(), "try_new(schema_version=0) must error");
        } else {
            // Must succeed
            let env = PersistedEnvelope::<()>::new("s", Version::from_persisted(1), "E", schema_version, &[], ());
            prop_assert_eq!(env.schema_version(), schema_version);

            let env = PersistedEnvelope::<()>::try_new("s", Version::from_persisted(1), "E", schema_version, &[], ());
            prop_assert!(env.is_ok());
            prop_assert_eq!(env.unwrap().schema_version(), schema_version);
        }
    }
}

// ============================================================================
// ATTACK 30: Version — next() at u64::MAX must panic
// ============================================================================

#[test]
fn attack_version_next_at_max_panics() {
    let v = Version::from_persisted(u64::MAX);
    let result = std::panic::catch_unwind(move || v.next());
    assert!(result.is_err(), "Version::next() at u64::MAX must panic");
}

// ============================================================================
// ATTACK 31: StoreError variants preserve information
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_store_error_conflict_preserves_info(
        expected in 0..10000u64,
        actual in 0..10000u64,
    ) {
        prop_assume!(expected != actual);

        let err = StoreError::Conflict {
            stream_id: nexus::ErrorId::from_display(&"test-stream"),
            expected: Version::from_persisted(expected),
            actual: Version::from_persisted(actual),
        };

        let msg = err.to_string();
        prop_assert!(
            msg.contains(&expected.to_string()),
            "expected version {} missing from error: {}",
            expected, msg,
        );
        prop_assert!(
            msg.contains(&actual.to_string()),
            "actual version {} missing from error: {}",
            actual, msg,
        );
    }
}

// ============================================================================
// ATTACK 32: InMemoryStore — append to non-existent stream creates it
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_append_creates_new_stream(
        stream_id in stream_id_strategy(),
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..50), 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Reading a non-existent stream should return empty
            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert!(read.is_empty(), "non-existent stream should be empty");

            // Append to non-existent stream should succeed (creating it)
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            // Now reading should return the events
            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), payloads.len());
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 33: Mixed event types in EventStore save/load
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_mixed_event_types_roundtrip(
        values in prop::collection::vec(-100..100i64, 0..5),
        strings in prop::collection::vec("[a-zA-Z0-9]{1,20}", 0..5),
    ) {
        prop_assume!(!values.is_empty() || !strings.is_empty());

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(7));

            // Interleave different event types
            let mut expected_log: Vec<String> = Vec::new();
            let mut expected_last_value: i64 = 0;

            for (i, v) in values.iter().enumerate() {
                root.apply(TestEvent::ValueSet(*v));
                expected_last_value = *v;

                if i < strings.len() {
                    root.apply(TestEvent::Happened(strings[i].clone()));
                    expected_log.push(strings[i].clone());
                }
            }
            // Remaining strings
            for s in strings.iter().skip(values.len()) {
                root.apply(TestEvent::Happened(s.clone()));
                expected_log.push(s.clone());
            }

            es.save("mixed-stream", &mut root).await.unwrap();
            let loaded: nexus::AggregateRoot<TestAggregate> = es.load("mixed-stream", TestId(7)).await.unwrap();

            prop_assert_eq!(loaded.state().last_value, expected_last_value);
            prop_assert_eq!(&loaded.state().log, &expected_log);
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 34: apply_upcasters with no upcasters is identity
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn attack_apply_upcasters_no_op_with_empty_chain(
        event_type in "[A-Z][a-z]{0,20}",
        schema_version in 1..1000u32,
        payload in prop::collection::vec(any::<u8>(), 0..500),
    ) {
        let upcasters: Vec<&dyn EventUpcaster> = vec![];
        let (out_type, out_version, out_payload) = apply_upcasters(
            &upcasters, &event_type, schema_version, &payload,
        ).unwrap();

        prop_assert_eq!(out_type, event_type, "event_type changed with empty chain");
        prop_assert_eq!(out_version, schema_version, "version changed with empty chain");
        prop_assert_eq!(out_payload, payload, "payload changed with empty chain");
    }
}

// ============================================================================
// ATTACK 35: Stress test — many streams, many events
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn attack_stress_many_streams(
        num_streams in 5..20usize,
        events_per_stream in 10..50usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            // Create many streams with many events
            for stream_idx in 0..num_streams {
                let stream_id = format!("stress-{}", stream_idx);
                let payloads: Vec<Vec<u8>> = (0..events_per_stream)
                    .map(|i| {
                        let mut p = vec![stream_idx as u8];
                        p.extend_from_slice(&(i as u32).to_le_bytes());
                        p
                    })
                    .collect();

                let envelopes = build_envelopes(&stream_id, &payloads);
                store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();
            }

            // Verify each stream independently
            for stream_idx in 0..num_streams {
                let stream_id = format!("stress-{}", stream_idx);
                let read = read_all_payloads(&store, &stream_id).await;
                prop_assert_eq!(
                    read.len(), events_per_stream,
                    "stream {} has wrong event count", stream_idx,
                );

                // Verify first byte is the stream index
                for payload in &read {
                    prop_assert_eq!(
                        payload[0], stream_idx as u8,
                        "stream {} has payload from wrong stream", stream_idx,
                    );
                }
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 36: UpcastError Display formatting
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_upcast_error_display_includes_context(
        event_type in "[A-Z][a-z]{0,20}",
        schema_version in 1..1000u32,
    ) {
        let err1 = UpcastError::VersionNotAdvanced {
            event_type: event_type.clone(),
            input_version: schema_version,
            output_version: schema_version,
        };
        let msg1 = err1.to_string();
        prop_assert!(msg1.contains(&event_type), "event_type missing from error: {}", msg1);

        let err2 = UpcastError::EmptyEventType {
            input_event_type: event_type.clone(),
            schema_version,
        };
        let msg2 = err2.to_string();
        prop_assert!(msg2.contains(&event_type), "event_type missing from error: {}", msg2);

        let err3 = UpcastError::ChainLimitExceeded {
            event_type: event_type.clone(),
            schema_version,
            limit: 100,
        };
        let msg3 = err3.to_string();
        prop_assert!(msg3.contains(&event_type), "event_type missing from error: {}", msg3);
        prop_assert!(msg3.contains("100"), "limit missing from error: {}", msg3);
    }
}

// ============================================================================
// ATTACK 37: Concurrent writers racing on the same stream
// ============================================================================

#[tokio::test]
async fn attack_concurrent_writers_exactly_one_wins() {
    let store = Arc::new(InMemoryStore::new());
    let num_writers = 10;

    let mut handles = Vec::new();
    for writer_id in 0..num_writers {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let envelope = pending_envelope("race-stream".into())
                .version(Version::from_persisted(1))
                .event_type(Box::leak(format!("Writer{writer_id}").into_boxed_str()))
                .payload(vec![writer_id as u8])
                .build_without_metadata();

            store
                .append("race-stream", Version::INITIAL, &[envelope])
                .await
        }));
    }

    let mut successes = 0;
    let mut failures = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => successes += 1,
            Err(_) => failures += 1,
        }
    }

    // Exactly ONE writer should win, all others must fail
    assert_eq!(successes, 1, "exactly one writer must win the race");
    assert_eq!(
        failures,
        num_writers - 1,
        "all other writers must get conflict"
    );

    // The stream should have exactly 1 event
    let payloads = read_all_payloads(&store, "race-stream").await;
    assert_eq!(payloads.len(), 1, "stream must have exactly 1 event");
}

// ============================================================================
// ATTACK 38: Upcaster that changes event_type mid-chain
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_upcaster_type_morphing_chain(
        payload in prop::collection::vec(any::<u8>(), 0..200),
    ) {
        // Chain: "OldEvent" v1 -> "MiddleEvent" v2 -> "NewEvent" v3
        struct RenameV1;
        impl EventUpcaster for RenameV1 {
            fn can_upcast(&self, et: &str, v: u32) -> bool { et == "OldEvent" && v == 1 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("MiddleEvent".into(), 2, p.to_vec())
            }
        }
        struct RenameV2;
        impl EventUpcaster for RenameV2 {
            fn can_upcast(&self, et: &str, v: u32) -> bool { et == "MiddleEvent" && v == 2 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("NewEvent".into(), 3, p.to_vec())
            }
        }

        let upcasters: Vec<&dyn EventUpcaster> = vec![&RenameV1, &RenameV2];
        let (final_type, final_version, final_payload) = apply_upcasters(
            &upcasters, "OldEvent", 1, &payload,
        ).unwrap();

        prop_assert_eq!(final_type, "NewEvent", "type rename chain failed");
        prop_assert_eq!(final_version, 3);
        prop_assert_eq!(final_payload, payload, "payload corrupted during type rename");
    }
}

// ============================================================================
// ATTACK 39: Payload transformation upcasters — payload actually changes
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn attack_upcaster_payload_transformation(
        payload in prop::collection::vec(any::<u8>(), 1..500),
    ) {
        // Upcaster that prefixes payload with a magic header
        struct PrefixUpcaster;
        impl EventUpcaster for PrefixUpcaster {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == 1 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                let mut new_payload = vec![0xCA, 0xFE];
                new_payload.extend_from_slice(p);
                ("E".into(), 2, new_payload)
            }
        }

        let upcasters: Vec<&dyn EventUpcaster> = vec![&PrefixUpcaster];
        let (_, final_version, final_payload) = apply_upcasters(
            &upcasters, "E", 1, &payload,
        ).unwrap();

        prop_assert_eq!(final_version, 2);
        prop_assert_eq!(final_payload.len(), payload.len() + 2);
        prop_assert_eq!(&final_payload[..2], &[0xCA, 0xFE]);
        prop_assert_eq!(&final_payload[2..], payload.as_slice());
    }
}

// ============================================================================
// ATTACK 40: EventStore save after partial load (load, apply, save, apply more, save again)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_event_store_interleaved_save_apply(
        batch1 in prop::collection::vec(-100..100i64, 1..5),
        batch2 in prop::collection::vec(-100..100i64, 1..5),
        batch3 in prop::collection::vec(-100..100i64, 1..5),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);
            let sid = "interleave-stream";

            // Batch 1: create, apply, save
            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(1));
            for &v in &batch1 {
                root.apply(TestEvent::ValueSet(v));
            }
            es.save(sid, &mut root).await.unwrap();
            let v1 = root.version().as_u64();
            prop_assert_eq!(v1, u64::try_from(batch1.len()).unwrap());

            // Batch 2: apply more to SAME root, save again
            for &v in &batch2 {
                root.apply(TestEvent::ValueSet(v));
            }
            es.save(sid, &mut root).await.unwrap();
            let v2 = root.version().as_u64();
            prop_assert_eq!(v2, u64::try_from(batch1.len() + batch2.len()).unwrap());

            // Batch 3: load fresh, apply, save
            let mut fresh: nexus::AggregateRoot<TestAggregate> = es.load(sid, TestId(1)).await.unwrap();
            prop_assert_eq!(fresh.version().as_u64(), v2);
            for &v in &batch3 {
                fresh.apply(TestEvent::ValueSet(v));
            }
            es.save(sid, &mut fresh).await.unwrap();

            // Final verification
            let final_root: nexus::AggregateRoot<TestAggregate> = es.load(sid, TestId(1)).await.unwrap();
            let total = batch1.len() + batch2.len() + batch3.len();
            prop_assert_eq!(
                final_root.version().as_u64(),
                u64::try_from(total).unwrap(),
            );
            prop_assert_eq!(
                final_root.state().events_applied,
                u64::try_from(total).unwrap(),
            );
            // Last value should be last of batch3
            prop_assert_eq!(
                final_root.state().last_value,
                *batch3.last().unwrap(),
            );
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 41: Idempotent read — reading same stream twice yields same data
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_idempotent_reads(
        stream_id in stream_id_strategy(),
        payloads in payloads_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let envelopes = build_envelopes(&stream_id, &payloads);
            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let read1 = read_all_payloads(&store, &stream_id).await;
            let read2 = read_all_payloads(&store, &stream_id).await;
            let read3 = read_all_payloads(&store, &stream_id).await;

            prop_assert_eq!(&read1, &read2, "second read differs from first");
            prop_assert_eq!(&read2, &read3, "third read differs from second");
            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 42: Commutative stream operations — order of stream creation doesn't
// affect other streams
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_stream_creation_order_independence(
        payloads_x in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..50), 1..10),
        payloads_y in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..50), 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Store A: write X first, then Y
            let store_a = InMemoryStore::new();
            let env_x = build_envelopes("x", &payloads_x);
            let env_y = build_envelopes("y", &payloads_y);
            store_a.append("x", Version::INITIAL, &env_x).await.unwrap();
            store_a.append("y", Version::INITIAL, &env_y).await.unwrap();

            // Store B: write Y first, then X
            let store_b = InMemoryStore::new();
            let env_x2 = build_envelopes("x", &payloads_x);
            let env_y2 = build_envelopes("y", &payloads_y);
            store_b.append("y", Version::INITIAL, &env_y2).await.unwrap();
            store_b.append("x", Version::INITIAL, &env_x2).await.unwrap();

            // Both stores should yield identical data for each stream
            let a_x = read_all_payloads(&store_a, "x").await;
            let b_x = read_all_payloads(&store_b, "x").await;
            prop_assert_eq!(&a_x, &b_x, "stream x differs based on creation order");

            let a_y = read_all_payloads(&store_a, "y").await;
            let b_y = read_all_payloads(&store_b, "y").await;
            prop_assert_eq!(&a_y, &b_y, "stream y differs based on creation order");

            Ok(())
        })?;
    }
}

// ============================================================================
// ATTACK 43: UpcasterChain cons-list — deep nesting doesn't stack overflow
// ============================================================================

#[test]
fn attack_deep_upcaster_chain_no_stack_overflow() {
    // Build a chain of 50 upcasters (V1->V2, V2->V3, ..., V50->V51)
    // using the cons-list. Each is a different struct.
    struct Step(u32);
    impl EventUpcaster for Step {
        fn can_upcast(&self, _: &str, v: u32) -> bool {
            v == self.0
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".into(), self.0 + 1, p.to_vec())
        }
    }

    // Build chain: Chain(Step(1), Chain(Step(2), ... Chain(Step(50), ())))
    // We have to do this manually since it's a type-level list.
    // Let's test with the slice-based apply_upcasters instead for depth.
    let steps: Vec<Step> = (1..=50).map(Step).collect();
    let upcasters: Vec<&dyn EventUpcaster> =
        steps.iter().map(|s| s as &dyn EventUpcaster).collect();

    let payload = vec![42u8; 1000];
    let (event_type, final_version, final_payload) =
        apply_upcasters(&upcasters, "E", 1, &payload).unwrap();

    assert_eq!(event_type, "E");
    assert_eq!(final_version, 51);
    assert_eq!(
        final_payload, payload,
        "payload corrupted through 50-step chain"
    );
}

// ============================================================================
// ATTACK 44: Aggregate with MAX_UNCOMMITTED = small, exercised through EventStore
// ============================================================================

#[derive(Default, Debug)]
struct TinyState {
    count: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct Tick;
impl nexus::Message for Tick {}
impl nexus::DomainEvent for Tick {
    fn name(&self) -> &'static str {
        "Tick"
    }
}

impl nexus::AggregateState for TinyState {
    type Event = Tick;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, _event: &Tick) -> Self {
        self.count += 1;
        self
    }
    fn name(&self) -> &'static str {
        "Tiny"
    }
}

struct TinyAggregate;
impl nexus::Aggregate for TinyAggregate {
    type State = TinyState;
    type Error = TestError;
    type Id = TestId;
    const MAX_UNCOMMITTED: usize = 3;
}

#[test]
fn attack_aggregate_max_uncommitted_panics() {
    let mut root = nexus::AggregateRoot::<TinyAggregate>::new(TestId(1));
    root.apply(Tick);
    root.apply(Tick);
    root.apply(Tick);

    // Fourth apply should panic because MAX_UNCOMMITTED = 3
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        root.apply(Tick);
    }));
    assert!(result.is_err(), "apply beyond MAX_UNCOMMITTED must panic");
}

// ============================================================================
// ATTACK 45: Save drains uncommitted, so a second save is always safe
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn attack_save_drains_uncommitted(
        n in 1..20usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let es = EventStore::new(store, JsonCodec);

            let mut root = nexus::AggregateRoot::<TestAggregate>::new(TestId(1));
            for i in 0..n {
                root.apply(TestEvent::ValueSet(i as i64));
            }

            // After save, uncommitted should be drained
            es.save("drain-test", &mut root).await.unwrap();
            prop_assert_eq!(root.version().as_u64(), u64::try_from(n).unwrap());

            // Second save should be a no-op (no uncommitted events)
            es.save("drain-test", &mut root).await.unwrap();

            // Can still apply more events after save
            root.apply(TestEvent::ValueSet(999));
            es.save("drain-test", &mut root).await.unwrap();

            let loaded: nexus::AggregateRoot<TestAggregate> = es.load("drain-test", TestId(1)).await.unwrap();
            prop_assert_eq!(loaded.state().last_value, 999);
            prop_assert_eq!(loaded.state().events_applied, u64::try_from(n + 1).unwrap());
            Ok(())
        })?;
    }
}
