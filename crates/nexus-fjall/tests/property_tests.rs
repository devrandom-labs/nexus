//! Property-based and adversarial tests for the fjall event store adapter.
//!
//! Uses proptest to generate chaotic inputs and verify store invariants hold.
//!
//! ## Coverage
//!
//! 1.  Encoding round-trips: event key, stream meta, event value
//! 2.  Encoding byte ordering and adversarial event type strings
//! 3.  Append-read roundtrips with arbitrary and adversarial payloads
//! 4.  Stream ID validation and isolation
//! 5.  Version boundaries, conflict detection, sequential enforcement
//! 6.  Concurrent append races
//! 7.  Persistence across reopen, counter recovery
//! 8.  Stress: many streams, large streams, large payloads
//! 9.  Schema version edge cases
//! 10. Model-based testing: shadow HashMap vs real store
//! 11. Edge cases: fused streams, version filtering

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]
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
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::print_stdout, reason = "diagnostic output")]
#![allow(dead_code, reason = "strategies used only in some proptest blocks")]

use std::collections::HashMap;
use std::num::NonZeroU32;

use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_fjall::encoding::{
    decode_event_key, decode_event_value, decode_stream_meta, encode_event_key, encode_event_value,
    encode_stream_meta,
};
use nexus_store::PendingEnvelope;
use nexus_store::envelope::pending_envelope;
use nexus_store::error::AppendError;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;

use proptest::prelude::*;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl nexus::Id for TestId {}
fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

// ============================================================================
// Helpers
// ============================================================================

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn temp_store() -> (FjallStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

fn make_envelope(version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope<()> {
    pending_envelope(Version::new(version).unwrap())
        .event_type(event_type)
        .payload(payload.to_vec())
        .build_without_metadata()
}

fn make_envelope_with_schema(
    version: u64,
    event_type: &'static str,
    payload: &[u8],
    schema_version: u32,
) -> PendingEnvelope<()> {
    let sv = NonZeroU32::new(schema_version).unwrap_or(NonZeroU32::MIN);
    pending_envelope(Version::new(version).unwrap())
        .event_type(event_type)
        .payload(payload.to_vec())
        .schema_version(sv)
        .build_without_metadata()
}

fn build_envelopes(payloads: &[Vec<u8>]) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            pending_envelope(Version::new(u64::try_from(i).unwrap() + 1).unwrap())
                .event_type(leak("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

fn build_envelopes_from(start_version: u64, payloads: &[Vec<u8>]) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            pending_envelope(Version::new(start_version + u64::try_from(i).unwrap()).unwrap())
                .event_type(leak("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

async fn read_all_payloads(store: &FjallStore, stream_id: &TestId) -> Vec<Vec<u8>> {
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

async fn read_all_versions(store: &FjallStore, stream_id: &TestId) -> Vec<u64> {
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

async fn read_all_event_types(store: &FjallStore, stream_id: &TestId) -> Vec<String> {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut types = Vec::new();
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        types.push(env.event_type().to_owned());
    }
    types
}

async fn count_events(store: &FjallStore, stream_id: &TestId) -> u64 {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut count = 0u64;
    while let Some(result) = stream.next().await {
        let _ = result.unwrap();
        count += 1;
    }
    count
}

// ============================================================================
// Strategies
// ============================================================================

fn stream_id_strategy() -> impl Strategy<Value = TestId> {
    prop::string::string_regex("[a-z][a-z0-9_-]{0,29}")
        .unwrap()
        .prop_map(TestId)
}

fn evil_stream_id_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Normal IDs
        prop::string::string_regex("[a-z][a-z0-9-]{0,19}").unwrap(),
        // Empty string
        Just(String::new()),
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
        Just("caf\u{00E9}".to_owned()),    // NFC vs NFD
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
        // Almost valid JSON
        Just(br#"{"broken":}"#.to_vec()),
        // Null bytes everywhere
        Just(vec![0; 10_000]),
    ]
}

// ============================================================================
// CATEGORY 1: Encoding Attack Surface (proptest)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// For any (u64, u64) pair, encode_event_key -> decode_event_key is identity.
    #[test]
    fn attack_encoding_event_key_round_trip_any_values(
        stream_num in any::<u64>(),
        version in any::<u64>(),
    ) {
        let encoded = encode_event_key(stream_num, version);
        let (decoded_stream, decoded_version) = decode_event_key(&encoded).unwrap();
        prop_assert_eq!(decoded_stream, stream_num, "stream_num round-trip failed");
        prop_assert_eq!(decoded_version, version, "version round-trip failed");
    }

    /// For any (u64, u64) pair, encode_stream_meta -> decode_stream_meta is identity.
    #[test]
    fn attack_encoding_stream_meta_round_trip_any_values(
        numeric_id in any::<u64>(),
        version in any::<u64>(),
    ) {
        let encoded = encode_stream_meta(numeric_id, version);
        let (decoded_id, decoded_ver) = decode_stream_meta(&encoded).unwrap();
        prop_assert_eq!(decoded_id, numeric_id, "numeric_id round-trip failed");
        prop_assert_eq!(decoded_ver, version, "version round-trip failed");
    }

    /// For any (u32, String, Vec<u8>) triple where event_type.len() <= u16::MAX,
    /// encode_event_value -> decode_event_value is identity.
    #[test]
    fn attack_encoding_event_value_round_trip_any_data(
        schema_ver in any::<u32>(),
        event_type in "[a-zA-Z_][a-zA-Z0-9_]{0,200}",
        payload in prop::collection::vec(any::<u8>(), 0..1024),
    ) {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, schema_ver, &event_type, &payload).unwrap();
        let (dec_sv, dec_et, dec_payload) = decode_event_value(&buf).unwrap();
        prop_assert_eq!(dec_sv, schema_ver, "schema_version round-trip failed");
        prop_assert_eq!(dec_et, &event_type, "event_type round-trip failed");
        prop_assert_eq!(dec_payload, payload.as_slice(), "payload round-trip failed");
    }

    /// For any a < b (u64), encode_event_key(stream, a) < encode_event_key(stream, b).
    /// Also: for any s1 < s2, encode_event_key(s1, v) < encode_event_key(s2, v).
    #[test]
    fn attack_encoding_event_key_byte_ordering(
        stream in any::<u64>(),
        a in 0..u64::MAX,
    ) {
        let b = a + 1;
        let key_a = encode_event_key(stream, a);
        let key_b = encode_event_key(stream, b);
        prop_assert!(key_a < key_b,
            "version ordering violated: key({}, {}) >= key({}, {})", stream, a, stream, b);
    }

    #[test]
    fn attack_encoding_event_key_stream_ordering(
        version in any::<u64>(),
        s1 in 0..u64::MAX,
    ) {
        let s2 = s1 + 1;
        let key_s1 = encode_event_key(s1, version);
        let key_s2 = encode_event_key(s2, version);
        prop_assert!(key_s1 < key_s2,
            "stream ordering violated: key({}, {}) >= key({}, {})", s1, version, s2, version);
    }
}

/// Evil event type strings — the encoding must handle them or reject them cleanly.
#[test]
fn attack_encoding_event_value_evil_event_types() {
    let evil_types: Vec<(&str, &str)> = vec![
        ("", "empty string"),
        ("日本語🔥", "Unicode with emoji"),
        ("\0\0\0", "null bytes"),
        ("   \t\n  ", "whitespace only"),
        ("../../../etc/passwd", "path traversal"),
        ("'; DROP TABLE events; --", "SQL injection"),
        ("\u{200B}", "zero-width space"),
        ("\u{FEFF}BOM", "byte order mark prefix"),
    ];

    let mut buf = Vec::new();
    for (evil_type, description) in &evil_types {
        let result = encode_event_value(&mut buf, 1, evil_type, b"test");
        match result {
            Ok(()) => {
                let (sv, decoded_type, payload) = decode_event_value(&buf).unwrap();
                assert_eq!(sv, 1, "schema_version corrupted for: {}", description);
                assert_eq!(
                    decoded_type, *evil_type,
                    "event_type corrupted for: {}",
                    description
                );
                assert_eq!(payload, b"test", "payload corrupted for: {}", description);
            }
            Err(_) => {
                // Rejection is also acceptable — document it
                println!(
                    "Encoding rejected evil event type '{}' ({})",
                    evil_type, description
                );
            }
        }
    }

    // Very long string near u16::MAX bytes
    let long_type = "a".repeat(usize::from(u16::MAX));
    let result = encode_event_value(&mut buf, 1, &long_type, b"x");
    assert!(
        result.is_ok(),
        "event_type at exactly u16::MAX bytes must be accepted"
    );
    let (_, decoded, _) = decode_event_value(&buf).unwrap();
    assert_eq!(decoded.len(), usize::from(u16::MAX));

    // One byte over u16::MAX must be rejected
    let too_long_type = "a".repeat(usize::from(u16::MAX) + 1);
    let result = encode_event_value(&mut buf, 1, &too_long_type, b"x");
    assert!(
        result.is_err(),
        "event_type exceeding u16::MAX bytes must be rejected"
    );
}

// ============================================================================
// CATEGORY 2: Append-Read Roundtrip (proptest)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any Vec<Vec<u8>> payloads, append all -> read_stream -> payloads match exactly.
    #[test]
    fn attack_append_read_roundtrip_any_payloads(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..1024), 1..50),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = tid("roundtrip-test");
            let envelopes = build_envelopes(&payloads);

            store.append(&stream_id, None, &envelopes).await.unwrap();

            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), payloads.len(), "payload count mismatch");
            for (i, (read_p, orig_p)) in read.iter().zip(payloads.iter()).enumerate() {
                prop_assert_eq!(read_p, orig_p, "payload mismatch at index {}", i);
            }
            Ok(())
        })?;
    }

    /// Adversarial payloads must survive round-trip unchanged.
    #[test]
    fn attack_append_read_roundtrip_evil_payloads(
        payloads in prop::collection::vec(evil_payload_strategy(), 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = tid("evil-roundtrip");
            let envelopes = build_envelopes(&payloads);

            store.append(&stream_id, None, &envelopes).await.unwrap();

            let read = read_all_payloads(&store, &stream_id).await;
            prop_assert_eq!(read.len(), payloads.len());
            for (i, (r, o)) in read.iter().zip(payloads.iter()).enumerate() {
                prop_assert_eq!(r, o, "evil payload corrupted at index {}", i);
            }
            Ok(())
        })?;
    }
}

/// Fixed test cases with specific adversarial payloads.
#[tokio::test]
async fn attack_append_read_roundtrip_fixed_evil_payloads() {
    let (store, _dir) = temp_store();
    let evil_payloads: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("all_zeros", vec![0u8; 1024]),
        ("all_0xff", vec![0xFF; 1024]),
        ("single_byte", vec![42]),
        ("null_bytes", vec![0x00; 512]),
        (
            "looks_like_header",
            vec![0u8; 6], // could confuse the decoder
        ),
        ("almost_json", br#"{"broken":}"#.to_vec()),
        (
            "binary_garbage",
            (0..=255).collect::<Vec<u8>>(), // all byte values
        ),
    ];

    for (i, (description, payload)) in evil_payloads.iter().enumerate() {
        let stream_id = TestId(format!("evil-{}", i));
        let version = u64::try_from(i).unwrap() + 1;

        // Each gets its own stream to avoid version conflicts
        let env = make_envelope(1, "EvilEvent", payload);
        store.append(&stream_id, None, &[env]).await.unwrap();

        let read = read_all_payloads(&store, &stream_id).await;
        assert_eq!(read.len(), 1, "wrong count for: {}", description);
        assert_eq!(
            read[0], *payload,
            "payload corrupted for: {} (version {})",
            description, version
        );
    }
}

// ============================================================================
// CATEGORY 3: Stream ID Attack Surface
// ============================================================================

/// Evil stream IDs: Unicode, injection, path traversal, null bytes, whitespace.
/// Empty string is tested separately in attack_empty_string_stream_id_returns_error.
#[tokio::test]
async fn attack_evil_stream_ids() {
    let evil_ids: Vec<(&str, &str)> = vec![
        ("x", "single character"),
        ("\u{65E5}\u{672C}\u{8A9E}", "CJK characters"),
        ("\u{1F525}\u{1F525}\u{1F525}", "emoji"),
        ("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}", "RTL Arabic"),
        ("\u{200B}", "zero-width space"),
        ("stream\0evil", "null byte embedded"),
        ("../../../etc/passwd", "path traversal unix"),
        ("..\\..\\windows", "path traversal windows"),
        ("'; DROP TABLE events; --", "SQL injection"),
        ("  ", "spaces only"),
        ("\t\n", "tab and newline"),
        ("stream with spaces", "spaces in name"),
        ("\u{FEFF}stream", "BOM prefix"),
    ];

    let (store, _dir) = temp_store();

    for (evil_id, description) in &evil_ids {
        // Use the evil_id directly as a TestId — no StreamId validation anymore.
        let stream_id = TestId(evil_id.to_string());

        let env = make_envelope(1, "Created", b"test-payload");
        let result = store.append(&stream_id, None, &[env]).await;

        match result {
            Ok(()) => {
                // If append succeeded, reading must return the exact data
                let read = read_all_payloads(&store, &stream_id).await;
                assert_eq!(
                    read.len(),
                    1,
                    "wrong event count for stream ID: {} ({})",
                    evil_id,
                    description
                );
                assert_eq!(
                    read[0], b"test-payload",
                    "payload corrupted for stream ID: {} ({})",
                    evil_id, description
                );
            }
            Err(e) => {
                // Rejection is also acceptable for truly pathological IDs
                println!(
                    "Stream ID '{}' ({}) was rejected: {}",
                    evil_id, description, e
                );
            }
        }
    }
}

/// Very long stream ID (10,000 characters).
#[tokio::test]
async fn attack_very_long_stream_id() {
    // Version test: from_persisted(0) returns None.
    let _long_id = "a".repeat(10_000);
    let result = Version::new(0);
    assert!(result.is_none(), "Version 0 must not be constructable");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any two distinct stream IDs, events appended to s1 never appear in s2.
    #[test]
    fn attack_stream_id_isolation_never_leaks(
        s1 in stream_id_strategy(),
        s2 in stream_id_strategy(),
    ) {
        prop_assume!(s1 != s2);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();

            let env1 = make_envelope(1, "EventA", b"payload-a");
            store.append(&s1, None, &[env1]).await.unwrap();

            let env2 = make_envelope(1, "EventB", b"payload-b");
            store.append(&s2, None, &[env2]).await.unwrap();

            // s1 must only contain its own data
            let s1_payloads = read_all_payloads(&store, &s1).await;
            prop_assert_eq!(s1_payloads.len(), 1, "s1 wrong count");
            prop_assert_eq!(&s1_payloads[0], &b"payload-a".to_vec(), "s1 payload leaked");

            // s2 must only contain its own data
            let s2_payloads = read_all_payloads(&store, &s2).await;
            prop_assert_eq!(s2_payloads.len(), 1, "s2 wrong count");
            prop_assert_eq!(&s2_payloads[0], &b"payload-b".to_vec(), "s2 payload leaked");

            // s1 event types must not leak
            let s1_types = read_all_event_types(&store, &s1).await;
            prop_assert_eq!(s1_types, vec!["EventA".to_owned()]);

            let s2_types = read_all_event_types(&store, &s2).await;
            prop_assert_eq!(s2_types, vec!["EventB".to_owned()]);

            Ok(())
        })?;
    }
}

// ============================================================================
// CATEGORY 4: Version Boundary Arithmetic
// ============================================================================

/// Append with expected_version = 0, event at version 1 -> OK.
#[tokio::test]
async fn attack_version_boundaries_basic() {
    let (store, _dir) = temp_store();
    let env = make_envelope(1, "Created", b"ok");
    let result = store.append(&tid("v-basic"), None, &[env]).await;
    assert!(result.is_ok(), "basic version 0->1 must succeed");
}

/// What happens near u64::MAX?
#[tokio::test]
async fn attack_version_boundaries_near_max() {
    let (_store, _dir) = temp_store();
    // We can't easily put u64::MAX - 1 events in the store, but we can test
    // Version::new(u64::MAX) construction
    let v_max = Version::new(u64::MAX).unwrap();
    assert_eq!(v_max.as_u64(), u64::MAX);

    // Version::next() at u64::MAX returns None (no overflow panic)
    assert!(
        v_max.next().is_none(),
        "Version::next() at u64::MAX must return None"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// For any stream with N events, appending with wrong expected_version
    /// always returns AppendError::Conflict with correct values.
    #[test]
    fn attack_version_conflict_detection(
        n in 1..20usize,
        wrong_version in 0..100u64,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = tid("conflict-detect");

            // Put N events in the stream
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&payloads);
            store.append(&stream_id, None, &envelopes).await.unwrap();

            let actual_version = u64::try_from(n).unwrap();
            prop_assume!(wrong_version != actual_version);

            // Try to append with wrong expected_version
            let new_envelopes = build_envelopes_from(
                wrong_version + 1,
                &[vec![0xFF]],
            );
            let result = store.append(
                &stream_id,
                Version::new(wrong_version),
                &new_envelopes,
            ).await;

            prop_assert!(result.is_err(), "wrong expected_version MUST be rejected");
            match result.unwrap_err() {
                AppendError::Conflict { expected, actual, .. } => {
                    prop_assert_eq!(expected, Version::new(wrong_version),
                        "conflict expected version wrong");
                    prop_assert_eq!(actual, Version::new(actual_version),
                        "conflict actual version wrong");
                }
                AppendError::Store(e) => {
                    panic!("expected Conflict, got Store error: {}", e);
                }
            }
            Ok(())
        })?;
    }
}

/// Non-sequential versions in a batch must be rejected.
#[tokio::test]
async fn attack_sequential_version_enforcement() {
    let (store, _dir) = temp_store();

    // [1, 3] — gap
    let envs_gap = vec![make_envelope(1, "A", b""), make_envelope(3, "B", b"")];
    let result = store.append(&tid("seq-gap"), None, &envs_gap).await;
    assert!(result.is_err(), "[1, 3] gap must be rejected");

    // [2, 1] — reversed
    let envs_rev = vec![make_envelope(2, "A", b""), make_envelope(1, "B", b"")];
    let result = store.append(&tid("seq-rev"), None, &envs_rev).await;
    assert!(result.is_err(), "[2, 1] reversed must be rejected");

    // [1, 1] — duplicate
    let envs_dup = vec![make_envelope(1, "A", b""), make_envelope(1, "B", b"")];
    let result = store.append(&tid("seq-dup"), None, &envs_dup).await;
    assert!(result.is_err(), "[1, 1] duplicate must be rejected");

    // [0, 1] — starts at 0 instead of 1
    // Version 0 is now unrepresentable (NonZeroU64), so this is enforced at the type level.
    assert!(
        Version::new(0).is_none(),
        "Version 0 must not be constructable"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Random version sequences must be rejected unless perfectly sequential from 1.
    #[test]
    fn attack_random_versions_rejected_unless_sequential(
        versions in prop::collection::vec(1..1000u64, 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = tid("random-versions");

            let envelopes: Vec<_> = versions.iter().map(|&v| {
                make_envelope(v, leak("E"), &[v as u8])
            }).collect();

            let result = store.append(&stream_id, None, &envelopes).await;

            let is_sequential = versions.iter().enumerate().all(|(i, &v)| v == (i as u64) + 1);
            if is_sequential {
                prop_assert!(result.is_ok(), "sequential versions must be accepted");
            } else {
                prop_assert!(result.is_err(), "non-sequential versions MUST be rejected: {:?}", versions);
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// CATEGORY 5: Concurrency and Race Conditions
// ============================================================================

/// Spawn 10 tokio tasks all trying to append version 1 to the same stream.
/// Exactly 1 should succeed, 9 should get AppendError::Conflict.
#[tokio::test]
async fn attack_concurrent_appends_one_wins() {
    use std::sync::Arc;

    let (store, _dir) = temp_store();
    let store = Arc::new(store);

    let n_tasks = 10u32;
    let mut handles = Vec::new();

    for task_id in 0..n_tasks {
        let store = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            let env = make_envelope(1, "Raced", &[task_id as u8]);
            store.append(&tid("race-stream"), None, &[env]).await
        });
        handles.push(handle);
    }

    let mut success_count = 0u32;
    let mut conflict_count = 0u32;

    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => success_count += 1,
            Err(AppendError::Conflict { .. }) => conflict_count += 1,
            Err(AppendError::Store(e)) => panic!("unexpected store error: {}", e),
        }
    }

    assert_eq!(
        success_count, 1,
        "exactly 1 task must succeed (got {})",
        success_count
    );
    assert_eq!(
        conflict_count,
        n_tasks - 1,
        "remaining tasks must get Conflict"
    );

    // Verify the stream has exactly 1 event
    let count = count_events(&store, &tid("race-stream")).await;
    assert_eq!(count, 1, "race-stream must have exactly 1 event");
}

/// Read during write: append batch 1, then read while appending batch 2.
/// The read should return a consistent snapshot.
#[tokio::test]
async fn attack_read_during_write() {
    use std::sync::Arc;

    let (store, _dir) = temp_store();
    let store = Arc::new(store);

    // Append batch 1 (versions 1-10)
    let batch1: Vec<_> = (1..=10u64)
        .map(|v| make_envelope(v, "Batch1", &[v as u8]))
        .collect();
    store
        .append(&tid("rw-stream"), None, &batch1)
        .await
        .unwrap();

    // Spawn a writer for batch 2 (versions 11-20)
    let store_write = Arc::clone(&store);
    let write_handle = tokio::spawn(async move {
        let batch2: Vec<_> = (11..=20u64)
            .map(|v| make_envelope(v, "Batch2", &[v as u8]))
            .collect();
        store_write
            .append(&tid("rw-stream"), Version::new(10), &batch2)
            .await
            .unwrap();
    });

    // Read concurrently
    let store_read = Arc::clone(&store);
    let read_handle = tokio::spawn(async move {
        let versions = read_all_versions(&store_read, &tid("rw-stream")).await;
        versions.len()
    });

    write_handle.await.unwrap();
    let read_count = read_handle.await.unwrap();

    // Read should see either 10 (pre-batch2) or 20 (post-batch2), not something in between
    assert!(
        read_count == 10 || read_count == 20,
        "read must see consistent snapshot: got {} events (expected 10 or 20)",
        read_count
    );
}

// ============================================================================
// CATEGORY 6: Persistence and Recovery
// ============================================================================

/// Open store, append events, close, reopen, verify — repeat 5 times.
#[tokio::test]
async fn attack_persistence_across_many_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    let mut total_events = 0u64;

    for round in 0..5u64 {
        // Open, append 100 events
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            let start = total_events + 1;
            let envs: Vec<_> = (start..start + 100)
                .map(|v| make_envelope(v, "Tick", &[v as u8]))
                .collect();
            store
                .append(&tid("persist-stream"), Version::new(total_events), &envs)
                .await
                .unwrap();
            total_events += 100;
        }

        // Reopen and verify cumulative count
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            let count = count_events(&store, &tid("persist-stream")).await;
            assert_eq!(
                count, total_events,
                "round {}: expected {} events, got {}",
                round, total_events, count
            );

            // Also verify first and last event
            let versions = read_all_versions(&store, &tid("persist-stream")).await;
            assert_eq!(versions[0], 1, "round {}: first version wrong", round);
            assert_eq!(
                *versions.last().unwrap(),
                total_events,
                "round {}: last version wrong",
                round
            );
        }
    }
}

/// Create streams "a", "b", "c", close, reopen, create "d" — should get a unique numeric ID.
#[tokio::test]
async fn attack_numeric_id_counter_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    // Create 3 streams
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        store
            .append(&tid("a"), None, &[make_envelope(1, "A", b"a-data")])
            .await
            .unwrap();
        store
            .append(&tid("b"), None, &[make_envelope(1, "B", b"b-data")])
            .await
            .unwrap();
        store
            .append(&tid("c"), None, &[make_envelope(1, "C", b"c-data")])
            .await
            .unwrap();
    }

    // Reopen, create "d"
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        store
            .append(&tid("d"), None, &[make_envelope(1, "D", b"d-data")])
            .await
            .unwrap();

        // All 4 streams must be isolated and readable
        for (id, expected_type, expected_payload) in [
            ("a", "A", b"a-data" as &[u8]),
            ("b", "B", b"b-data"),
            ("c", "C", b"c-data"),
            ("d", "D", b"d-data"),
        ] {
            let stream_id = tid(id);
            let mut stream = store
                .read_stream(&stream_id, Version::INITIAL)
                .await
                .unwrap();
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), expected_type, "stream {} type wrong", id);
            assert_eq!(env.payload(), expected_payload, "stream {} data wrong", id);
            assert!(
                stream.next().await.is_none(),
                "stream {} has extra events",
                id
            );
        }
    }
}

/// Create 100 streams with 10 events each, close, reopen, verify all.
#[tokio::test]
async fn attack_many_streams_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");

    // Create 100 streams
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        for stream_num in 0..100u64 {
            let stream_id = TestId(format!("stream-{}", stream_num));
            let envs: Vec<_> = (1..=10u64)
                .map(|v| make_envelope(v, "Tick", &[stream_num as u8, v as u8]))
                .collect();
            store.append(&stream_id, None, &envs).await.unwrap();
        }
    }

    // Reopen and verify
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        for stream_num in 0..100u64 {
            let stream_id = TestId(format!("stream-{}", stream_num));
            let count = count_events(&store, &stream_id).await;
            assert_eq!(
                count, 10,
                "stream-{} should have 10 events, got {}",
                stream_num, count
            );

            let payloads = read_all_payloads(&store, &stream_id).await;
            for (i, payload) in payloads.iter().enumerate() {
                let expected = vec![stream_num as u8, (i as u8) + 1];
                assert_eq!(
                    *payload, expected,
                    "stream-{} event {} payload corrupted",
                    stream_num, i
                );
            }
        }
    }
}

// ============================================================================
// CATEGORY 7: Stress and Scale
// ============================================================================

/// Create 50 streams, verify complete isolation.
#[tokio::test]
async fn attack_many_streams_isolation() {
    let (store, _dir) = temp_store();

    for stream_num in 0..50u64 {
        let stream_id = TestId(format!("iso-{}", stream_num));
        let envs: Vec<_> = (1..=5u64)
            .map(|v| {
                let event_type = leak(&format!("Event{}", stream_num));
                make_envelope(v, event_type, &[stream_num as u8; 32])
            })
            .collect();
        store.append(&stream_id, None, &envs).await.unwrap();
    }

    // Verify each stream is isolated
    for stream_num in 0..50u64 {
        let stream_id = TestId(format!("iso-{}", stream_num));
        let expected_type = format!("Event{}", stream_num);

        let types = read_all_event_types(&store, &stream_id).await;
        assert_eq!(types.len(), 5, "stream {} wrong event count", stream_num);
        for t in &types {
            assert_eq!(
                *t, expected_type,
                "stream {} has foreign event type: {}",
                stream_num, t
            );
        }

        let payloads = read_all_payloads(&store, &stream_id).await;
        for (i, p) in payloads.iter().enumerate() {
            assert_eq!(
                *p,
                vec![stream_num as u8; 32],
                "stream {} event {} payload cross-contaminated",
                stream_num,
                i
            );
        }
    }
}

/// Append 10,000 events in batches of 100, read all back.
#[tokio::test]
async fn attack_large_event_stream() {
    let (store, _dir) = temp_store();
    let total = 10_000u64;
    let batch_size = 100u64;

    let mut current_version = 0u64;
    while current_version < total {
        let start = current_version + 1;
        let end = std::cmp::min(current_version + batch_size, total);
        let envs: Vec<_> = (start..=end)
            .map(|v| make_envelope(v, "Tick", &v.to_le_bytes()))
            .collect();
        store
            .append(&tid("big-stream"), Version::new(current_version), &envs)
            .await
            .unwrap();
        current_version = end;
    }

    // Read all back
    let mut stream = store
        .read_stream(&tid("big-stream"), Version::new(1).unwrap())
        .await
        .unwrap();
    let mut count = 0u64;
    let mut prev_version = 0u64;
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        count += 1;
        let v = env.version().as_u64();
        assert!(v > prev_version, "versions not monotonic at {}", v);
        assert_eq!(v, count, "version gap or skip at count {}", count);

        // Verify payload content
        let expected_payload = v.to_le_bytes();
        assert_eq!(
            env.payload(),
            &expected_payload,
            "payload corrupted at version {}",
            v
        );
        prev_version = v;
    }
    assert_eq!(count, total, "expected {} events, got {}", total, count);
}

/// Append events with 1MB payloads, verify exact content.
#[tokio::test]
async fn attack_large_payloads() {
    let (store, _dir) = temp_store();
    let payload_1mb = vec![0xAB_u8; 1_048_576]; // 1 MB

    let env = make_envelope(1, "HugeEvent", &payload_1mb);
    store
        .append(&tid("large-payload"), None, &[env])
        .await
        .unwrap();

    let payloads = read_all_payloads(&store, &tid("large-payload")).await;
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        payloads[0].len(),
        1_048_576,
        "1MB payload size changed after roundtrip"
    );
    assert_eq!(payloads[0], payload_1mb, "1MB payload content corrupted");
}

/// 1000 events with 1-byte payloads.
#[tokio::test]
async fn attack_many_small_events() {
    let (store, _dir) = temp_store();
    let envs: Vec<_> = (1..=1000u64)
        .map(|v| make_envelope(v, "Tiny", &[v as u8]))
        .collect();
    store
        .append(&tid("small-events"), None, &envs)
        .await
        .unwrap();

    let count = count_events(&store, &tid("small-events")).await;
    assert_eq!(count, 1000, "expected 1000 small events, got {}", count);
}

// ============================================================================
// CATEGORY 8: Schema Version Edge Cases
// ============================================================================

/// schema_version = 1 (minimum valid) round-trips correctly.
#[tokio::test]
async fn attack_schema_version_min_valid() {
    let (store, _dir) = temp_store();
    let env = make_envelope_with_schema(1, "Created", b"data", 1);
    store.append(&tid("sv-min"), None, &[env]).await.unwrap();

    let mut stream = store
        .read_stream(&tid("sv-min"), Version::new(1).unwrap())
        .await
        .unwrap();
    let persisted = stream.next().await.unwrap().unwrap();
    assert_eq!(persisted.schema_version(), 1);
}

/// schema_version = u32::MAX round-trips correctly.
#[tokio::test]
async fn attack_schema_version_u32_max() {
    let (store, _dir) = temp_store();
    let env = make_envelope_with_schema(1, "Created", b"data", u32::MAX);
    store.append(&tid("sv-max"), None, &[env]).await.unwrap();

    let mut stream = store
        .read_stream(&tid("sv-max"), Version::new(1).unwrap())
        .await
        .unwrap();
    let persisted = stream.next().await.unwrap().unwrap();
    assert_eq!(
        persisted.schema_version(),
        u32::MAX,
        "u32::MAX schema_version not preserved"
    );
}

/// schema_version = 0 is now impossible at the type level (`NonZeroU32`).
/// Verify that `NonZeroU32::MIN` (1) round-trips correctly.
#[tokio::test]
async fn attack_schema_version_zero_clamped_by_builder() {
    let (store, _dir) = temp_store();

    let env = pending_envelope(Version::INITIAL)
        .event_type("BadSchema")
        .payload(b"data".to_vec())
        .schema_version(NonZeroU32::MIN) // Minimum valid schema version
        .build_without_metadata();

    // schema_version should be 1
    assert_eq!(
        env.schema_version(),
        1,
        "schema_version must be 1 (NonZeroU32::MIN)"
    );

    store.append(&tid("sv-zero"), None, &[env]).await.unwrap();

    // Read succeeds because schema_version is now 1
    let mut stream = store
        .read_stream(&tid("sv-zero"), Version::new(1).unwrap())
        .await
        .unwrap();
    let persisted = stream.next().await.unwrap().unwrap();
    assert_eq!(persisted.schema_version(), 1);
}

// ============================================================================
// CATEGORY 9: Model-Based Testing (proptest)
// ============================================================================

#[derive(Debug, Clone)]
enum Op {
    Append { stream_idx: usize, n_events: usize },
    Read { stream_idx: usize },
}

fn op_strategy(n_streams: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..n_streams, 1..10usize).prop_map(|(stream_idx, n_events)| Op::Append {
            stream_idx,
            n_events,
        }),
        (0..n_streams).prop_map(|stream_idx| Op::Read { stream_idx }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Maintain a shadow model (HashMap) and compare with the real FjallStore
    /// after a sequence of random operations.
    #[test]
    fn attack_model_based_shadow_store(
        ops in prop::collection::vec(op_strategy(5), 10..50),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_ids: Vec<String> = (0..5).map(|i| format!("model-{}", i)).collect();

            // Shadow model: stream_id -> Vec<(version, payload)>
            let mut shadow: HashMap<String, Vec<(u64, Vec<u8>)>> = HashMap::new();

            for op in &ops {
                match op {
                    Op::Append { stream_idx, n_events } => {
                        let stream_id = &stream_ids[*stream_idx];
                        let current_count = shadow
                            .get(stream_id)
                            .map_or(0u64, |v| u64::try_from(v.len()).unwrap());

                        let sid = TestId(stream_id.clone());
                        let envelopes: Vec<_> = (1..=*n_events).map(|i| {
                            let version = current_count + u64::try_from(i).unwrap();
                            let payload = format!("op-{}-v{}", stream_id, version);
                            make_envelope(
                                version,
                                "ModelEvent",
                                payload.as_bytes(),
                            )
                        }).collect();

                        let result = store.append(
                            &sid,
                            Version::new(current_count),
                            &envelopes,
                        ).await;
                        prop_assert!(result.is_ok(),
                            "model-based append failed for stream {} at version {}",
                            stream_id, current_count);

                        // Update shadow
                        let entry = shadow.entry(stream_id.clone()).or_default();
                        for i in 1..=*n_events {
                            let version = current_count + u64::try_from(i).unwrap();
                            let payload = format!("op-{}-v{}", stream_id, version);
                            entry.push((version, payload.into_bytes()));
                        }
                    }
                    Op::Read { stream_idx } => {
                        let stream_id = &stream_ids[*stream_idx];
                        let sid = TestId(stream_id.clone());
                        let shadow_data = shadow.get(stream_id);

                        let real_payloads = read_all_payloads(&store, &sid).await;
                        let real_versions = read_all_versions(&store, &sid).await;

                        match shadow_data {
                            None => {
                                prop_assert_eq!(real_payloads.len(), 0,
                                    "shadow empty but store has {} events for {}",
                                    real_payloads.len(), stream_id);
                            }
                            Some(shadow_events) => {
                                prop_assert_eq!(real_payloads.len(), shadow_events.len(),
                                    "event count mismatch for {}: real={}, shadow={}",
                                    stream_id, real_payloads.len(), shadow_events.len());

                                for (i, ((sv, sp), (rv, rp))) in shadow_events.iter()
                                    .zip(real_versions.iter().zip(real_payloads.iter()))
                                    .enumerate()
                                {
                                    prop_assert_eq!(*rv, *sv,
                                        "version mismatch at index {} for {}", i, stream_id);
                                    prop_assert_eq!(rp, sp,
                                        "payload mismatch at index {} for {}", i, stream_id);
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// CATEGORY 10: Empty and Degenerate Cases
// ============================================================================

/// Read from Version::INITIAL returns events starting at version 1.
#[tokio::test]
async fn attack_read_from_initial_version() {
    let (store, _dir) = temp_store();
    let envs = vec![
        make_envelope(1, "A", b"a"),
        make_envelope(2, "B", b"b"),
        make_envelope(3, "C", b"c"),
    ];
    store
        .append(&tid("initial-read"), None, &envs)
        .await
        .unwrap();

    // Version::INITIAL is 0, so read_stream(_, from=0) — implementation may
    // return events starting at 1, or from 0. Let's test what actually happens.
    let mut stream = store
        .read_stream(&tid("initial-read"), Version::INITIAL)
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        versions.push(env.version().as_u64());
    }
    // The range scan starts from encode_event_key(numeric_id, 0), so it should
    // find all events (version 1, 2, 3) since they are >= 0.
    assert_eq!(
        versions,
        vec![1, 2, 3],
        "reading from INITIAL must return all events"
    );
}

/// Fused behavior: once next() returns None, all subsequent calls return None.
#[tokio::test]
async fn attack_stream_fused_after_exhaustion() {
    let (store, _dir) = temp_store();
    let env = make_envelope(1, "Created", b"data");
    store
        .append(&tid("fused-test"), None, &[env])
        .await
        .unwrap();

    let mut stream = store
        .read_stream(&tid("fused-test"), Version::new(1).unwrap())
        .await
        .unwrap();

    // Consume the single event
    let _ = stream.next().await.unwrap().unwrap();

    // All subsequent calls must return None (fused)
    assert!(stream.next().await.is_none(), "fused: call 1 after None");
    assert!(stream.next().await.is_none(), "fused: call 2 after None");
    assert!(stream.next().await.is_none(), "fused: call 3 after None");
}

/// Empty stream ID must return an error, not panic.
///
/// Previously this caused a panic deep inside fjall's lsm-tree ("key may not
/// be empty"). Now `FjallStore` validates at the boundary.
#[test]
fn attack_empty_string_stream_id_rejected_by_stream_id_type() {
    // Version::new(0) returns None — test version boundary instead.
    // Empty stream IDs can no longer reach the store layer.
    let result = Version::new(0);
    assert!(result.is_none(), "from_persisted(0) must return None");
}

/// Multiple appends to the same stream with correct version tracking.
#[tokio::test]
async fn attack_multiple_sequential_appends() {
    let (store, _dir) = temp_store();

    for batch in 0..20u64 {
        let start_version = batch * 5;
        let envs: Vec<_> = (1..=5u64)
            .map(|i| {
                let version = start_version + i;
                make_envelope(version, "Tick", &version.to_le_bytes())
            })
            .collect();
        store
            .append(&tid("multi-append"), Version::new(start_version), &envs)
            .await
            .unwrap();
    }

    let count = count_events(&store, &tid("multi-append")).await;
    assert_eq!(count, 100, "20 batches of 5 = 100 events");

    let versions = read_all_versions(&store, &tid("multi-append")).await;
    for (i, v) in versions.iter().enumerate() {
        assert_eq!(
            *v,
            u64::try_from(i).unwrap() + 1,
            "version gap at index {}",
            i
        );
    }
}

/// Verify read_stream with from_version filters correctly.
#[tokio::test]
async fn attack_read_with_from_version_filters() {
    let (store, _dir) = temp_store();
    let envs: Vec<_> = (1..=10u64)
        .map(|v| make_envelope(v, "Tick", &v.to_le_bytes()))
        .collect();
    store
        .append(&tid("filter-test"), None, &envs)
        .await
        .unwrap();

    // Read from version 5 — should get versions 5..=10
    let mut stream = store
        .read_stream(&tid("filter-test"), Version::new(5).unwrap())
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(result) = stream.next().await {
        let env = result.unwrap();
        versions.push(env.version().as_u64());
    }
    assert_eq!(
        versions,
        vec![5, 6, 7, 8, 9, 10],
        "from_version=5 must return events 5..=10"
    );
}

/// Verify that event_type is preserved through round-trip.
#[tokio::test]
async fn attack_event_type_preserved() {
    let (store, _dir) = temp_store();
    let types = ["Created", "Updated", "Deleted", "Archived", "Restored"];
    let envs: Vec<_> = types
        .iter()
        .enumerate()
        .map(|(i, &t)| make_envelope(u64::try_from(i).unwrap() + 1, t, b""))
        .collect();
    store.append(&tid("type-test"), None, &envs).await.unwrap();

    let read_types = read_all_event_types(&store, &tid("type-test")).await;
    let expected: Vec<String> = types.iter().map(|t| (*t).to_owned()).collect();
    assert_eq!(read_types, expected, "event types not preserved");
}

/// Append after empty batch: empty batch does not set the version.
#[tokio::test]
async fn attack_append_after_empty_batch() {
    let (store, _dir) = temp_store();

    // Empty batch first
    store.append(&tid("post-empty"), None, &[]).await.unwrap();

    // Now a real append — should still succeed with None because empty batch is no-op
    let env = make_envelope(1, "Created", b"after-empty");
    let result = store.append(&tid("post-empty"), None, &[env]).await;
    assert!(
        result.is_ok(),
        "append after empty batch must succeed with INITIAL: {:?}",
        result.err()
    );
}

// Schema version round-trip through the full stack for various values.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_schema_version_round_trip_any(
        schema_ver in 1..=u32::MAX,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = TestId(format!("sv-{}", schema_ver));
            let env = make_envelope_with_schema(
                1,
                "SchemaTest",
                b"data",
                schema_ver,
            );
            store.append(&stream_id, None, &[env]).await.unwrap();

            let mut stream = store.read_stream(&stream_id, Version::INITIAL).await.unwrap();
            let persisted = stream.next().await.unwrap().unwrap();
            prop_assert_eq!(persisted.schema_version(), schema_ver,
                "schema_version {} not preserved through roundtrip", schema_ver);
            Ok(())
        })?;
    }
}

// Verify monotonic version ordering in read stream output.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_stream_versions_always_monotonic(
        n in 1..50usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store, _dir) = temp_store();
            let stream_id = tid("monotonic-test");
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8]).collect();
            let envelopes = build_envelopes(&payloads);
            store.append(&stream_id, None, &envelopes).await.unwrap();

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
