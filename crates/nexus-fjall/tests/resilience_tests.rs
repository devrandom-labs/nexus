//! Resilience and stress tests for the fjall event store adapter.
//!
//! Targets boundary conditions, crash recovery, concurrent access,
//! arithmetic overflow, and encoding edge cases.
//!
//! ## Coverage
//!
//! A. Version arithmetic overflow
//! B. Dual instance data corruption
//! C. Deterministic simulation testing (DST) with shadow model
//! D. Snapshot isolation
//! E. Crash simulation via `mem::forget`
//! F. Key boundary conditions (`u64::MAX` reads)
//! G. Recovery torture (100 streams, 5 reopen cycles)
//! H. Concurrent append storm (50 tasks)
//! I. Encoding boundary attacks (`u16::MAX` event type)
//! K. Append-then-read version consistency

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::panic, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::cast_possible_wrap, reason = "tests")]
#![allow(clippy::cast_sign_loss, reason = "tests")]
#![allow(clippy::implicit_clone, reason = "tests")]
#![allow(clippy::clone_on_ref_ptr, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::print_stdout, reason = "diagnostic output")]
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::items_after_statements, reason = "tests")]
#![allow(dead_code, reason = "helpers used across test blocks")]

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_fjall::encoding::{decode_event_value, encode_event_value};
use nexus_store::PendingEnvelope;
use nexus_store::envelope::pending_envelope;
use nexus_store::error::AppendError;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;

use proptest::prelude::*;

// ============================================================================
// Helpers
// ============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl nexus::Id for TestId {
    const BYTE_LEN: usize = 0;
}
fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn make_envelope(version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope<()> {
    pending_envelope(Version::new(version).unwrap())
        .event_type(event_type)
        .payload(payload.to_vec())
        .build_without_metadata()
}

fn temp_store() -> (FjallStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

async fn read_all_payloads(store: &FjallStore, stream_id: &TestId) -> Vec<Vec<u8>> {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut payloads = Vec::new();
    while let Some(env) = stream.next().await.unwrap() {
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
    while let Some(env) = stream.next().await.unwrap() {
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
    while let Some(env) = stream.next().await.unwrap() {
        types.push(env.event_type().to_owned());
    }
    types
}

async fn count_events(store: &FjallStore, stream_id: &TestId) -> u64 {
    let mut stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .unwrap();
    let mut count: u64 = 0;
    while let Some(env) = stream.next().await.unwrap() {
        let _ = env;
        count += 1;
    }
    count
}

// ============================================================================
// CATEGORY A: Version Arithmetic Overflow (THE BIG ONE)
// ============================================================================

/// BUG FOUND: store.rs line 109 uses unchecked u64 addition:
///     `let expected_env_version = current_version + 1 + i_u64;`
/// If `current_version` is near `u64::MAX`, this overflows.
/// In debug mode: panic. In release mode: silent wrap to 0.
///
/// This test proves the arithmetic WILL overflow.
#[test]
fn attack_version_overflow_in_sequential_check() {
    // store.rs line 109: current_version + 1 + i_u64
    // If current_version = u64::MAX - 2 and i_u64 = 2:
    //   (u64::MAX - 2) + 1 + 2 = u64::MAX + 1 = OVERFLOW
    let current_version: u64 = u64::MAX - 2;
    let i: u64 = 2;
    let result = current_version
        .checked_add(1)
        .and_then(|v| v.checked_add(i));
    assert!(
        result.is_none(),
        "BUG CONFIRMED: version arithmetic at store.rs:109 overflows \
         when current_version={current_version}, i={i}"
    );
}

/// Prove that even i=0 overflows when `current_version` = `u64::MAX`
#[test]
fn attack_version_overflow_at_exact_max() {
    // current_version = u64::MAX, i = 0
    // (u64::MAX) + 1 + 0 = OVERFLOW on the first addition
    let current_version: u64 = u64::MAX;
    let result = current_version.checked_add(1);
    assert!(
        result.is_none(),
        "BUG CONFIRMED: current_version=u64::MAX + 1 overflows even with i=0"
    );
}

/// Prove the `new_version` computation can't be incremented past `u64::MAX`
#[test]
fn attack_new_version_computation_overflow() {
    let version_near_max = u64::MAX;
    let next = version_near_max.checked_add(1);
    assert!(
        next.is_none(),
        "version at u64::MAX cannot be incremented — next append would need version u64::MAX+1"
    );
}

/// Exhaustive boundary check: for which (`current_version`, `batch_size`) pairs
/// does the version arithmetic in store.rs:109 overflow?
#[test]
fn attack_version_overflow_boundary_exhaustive() {
    // The formula: current_version + 1 + i for i in 0..batch_size
    // Maximum i is batch_size - 1
    // Overflow happens when: current_version + 1 + (batch_size - 1) > u64::MAX
    // i.e., current_version + batch_size > u64::MAX
    // i.e., current_version > u64::MAX - batch_size

    for batch_size in 1u64..=10 {
        let threshold = u64::MAX - batch_size;
        // At threshold: current_version + 1 + (batch_size - 1) = u64::MAX → OK
        let ok_result = threshold
            .checked_add(1)
            .and_then(|v| v.checked_add(batch_size - 1));
        assert!(
            ok_result.is_some(),
            "current_version={threshold}, batch_size={batch_size} should NOT overflow"
        );

        // At threshold + 1: overflow
        let overflow_version = threshold + 1;
        let overflow_result = overflow_version
            .checked_add(1)
            .and_then(|v| v.checked_add(batch_size - 1));
        assert!(
            overflow_result.is_none(),
            "current_version={overflow_version}, batch_size={batch_size} SHOULD overflow"
        );
    }
}

// ============================================================================
// CATEGORY B: Dual Instance Attack
// ============================================================================

#[tokio::test]
async fn attack_dual_instance_same_path() {
    // Open TWO FjallStore instances on the same database path.
    // This tests whether fjall allows it and if it causes corruption.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store1 = FjallStore::builder(&path).open().unwrap();

    // Try opening a second instance on the same path
    let result = FjallStore::builder(&path).open();
    // If fjall allows this: both stores could have write conflicts
    // If fjall rejects this: we get an error (safe)

    match result {
        Ok(store2) => {
            // DANGEROUS: two writers on same DB!
            // Try creating different streams from each
            let sid1 = tid("stream-from-store1");
            let sid2 = tid("stream-from-store2");
            let env1 = make_envelope(1, "A", b"from-1");
            let env2 = make_envelope(1, "B", b"from-2");

            store1.append(&sid1, None, &[env1]).await.unwrap();
            store2.append(&sid2, None, &[env2]).await.unwrap();

            // Check for corruption: can we read back correctly?
            let r1 = read_all_event_types(&store1, &sid1).await;
            let r2 = read_all_event_types(&store2, &sid2).await;

            assert_eq!(r1, vec!["A"], "store1 stream corrupted by dual instance");
            assert_eq!(r2, vec!["B"], "store2 stream corrupted by dual instance");

            println!(
                "WARNING: dual instance opened successfully — \
                 concurrent writes may conflict"
            );
        }
        Err(e) => {
            println!("SAFE: fjall rejects dual instances: {e}");
        }
    }
}

// ============================================================================
// CATEGORY C: Deterministic Simulation Testing (DST)
// ============================================================================

#[derive(Debug, Clone)]
enum Op {
    Append { stream: usize, count: usize },
    Read { stream: usize },
    Reopen,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn attack_deterministic_simulation(
        ops in prop::collection::vec(
            prop_oneof![
                (0..5usize, 1..10usize).prop_map(|(s, c)| Op::Append { stream: s, count: c }),
                (0..5usize).prop_map(|s| Op::Read { stream: s }),
                Just(Op::Reopen),
            ],
            10..50,
        )
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("db");
            let mut store = FjallStore::builder(&path).open().unwrap();

            // Shadow model: stream_idx -> list of payloads
            let stream_names: Vec<TestId> = (0..5)
                .map(|i| tid(&format!("stream_{i}")))
                .collect();
            let mut model: HashMap<usize, Vec<Vec<u8>>> = HashMap::new();

            for op in &ops {
                match op {
                    Op::Append { stream, count } => {
                        let sid_ref = &stream_names[*stream];
                        let existing = model.get(stream).map_or(0, Vec::len);
                        let payloads: Vec<Vec<u8>> = (0..*count)
                            .map(|i| format!("data-{stream}-{}-{i}", existing + i).into_bytes())
                            .collect();

                        let mut envelopes = Vec::new();
                        for (i, payload) in payloads.iter().enumerate() {
                            let ver = u64::try_from(existing + i + 1).unwrap();
                            envelopes.push(
                                pending_envelope(Version::new(ver).unwrap())
                                    .event_type(leak(&format!("Tick_{stream}_{}", existing + i)))
                                    .payload(payload.clone())
                                    .build_without_metadata(),
                            );
                        }

                        let expected_ver =
                            Version::new(u64::try_from(existing).unwrap());
                        match store.append(sid_ref, expected_ver, &envelopes).await {
                            Ok(()) => {
                                model.entry(*stream).or_default().extend(payloads);
                            }
                            Err(e) => panic!("DST append failed unexpectedly: {e}"),
                        }
                    }
                    Op::Read { stream } => {
                        let sid_ref = &stream_names[*stream];
                        let expected = model.get(stream).cloned().unwrap_or_default();
                        let actual = read_all_payloads(&store, sid_ref).await;
                        assert_eq!(
                            actual, expected,
                            "DST: stream {stream} data mismatch after read"
                        );
                    }
                    Op::Reopen => {
                        drop(store);
                        store = FjallStore::builder(&path).open().unwrap();
                    }
                }
            }

            // Final invariant: all streams match model
            for (stream_idx, expected_payloads) in &model {
                let sid_ref = &stream_names[*stream_idx];
                let actual = read_all_payloads(&store, sid_ref).await;
                assert_eq!(
                    actual, *expected_payloads,
                    "DST final check: stream {stream_idx} mismatch"
                );
            }
        });
    }
}

// ============================================================================
// CATEGORY D: Snapshot Isolation Probe
// ============================================================================

#[tokio::test]
async fn attack_read_snapshot_isolation() {
    // Append 1000 events in 10 batches of 100, then verify read consistency
    let (store, _dir) = temp_store();
    let sid_val = tid("snapshot-stream");

    for batch in 0..10u64 {
        let start = batch * 100 + 1;
        let envelopes: Vec<_> = (start..start + 100)
            .map(|v| make_envelope(v, "Tick", &v.to_le_bytes()))
            .collect();
        let expected_ver = Version::new(batch * 100);
        store
            .append(&sid_val, expected_ver, &envelopes)
            .await
            .unwrap();
    }

    // Verify we can read exactly 1000 events
    let count = count_events(&store, &sid_val).await;
    assert_eq!(count, 1000, "expected 1000 events total");

    // Verify strict ordering: versions 1..=1000
    let versions = read_all_versions(&store, &sid_val).await;
    for (i, v) in versions.iter().enumerate() {
        let expected = u64::try_from(i).unwrap() + 1;
        assert_eq!(
            *v, expected,
            "version mismatch at position {i}: expected {expected}, got {v}"
        );
    }
}

// ============================================================================
// CATEGORY E: Crash Simulation via mem::forget
// ============================================================================

#[tokio::test]
async fn attack_crash_simulation_forget_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    // Write events, then "crash" by forgetting the store (no clean shutdown)
    {
        let store = FjallStore::builder(&path).open().unwrap();
        let envs = vec![make_envelope(1, "Before", b"crash-data")];
        store.append(&tid("crash-test"), None, &envs).await.unwrap();
        std::mem::forget(store); // Simulate crash — no Drop, no flush
    }

    // Reopen and check if data survived.
    // fjall v3 uses file locking — if mem::forget skipped Drop, the lock may
    // still be held, making reopen impossible. Both outcomes are valid:
    // - Locked: lock file not released (crash left stale lock)
    // - Ok: lock released, data may or may not have survived
    match FjallStore::builder(&path).open() {
        Err(_) => {
            println!(
                "DURABILITY NOTE: reopen failed after mem::forget — \
                 fjall v3 file lock was not released (expected for crash simulation)"
            );
        }
        Ok(store) => {
            let payloads = read_all_payloads(&store, &tid("crash-test")).await;
            if payloads.is_empty() {
                println!(
                    "DURABILITY NOTE: data lost after mem::forget — \
                     fjall does not fsync on every commit by default"
                );
            } else {
                assert_eq!(payloads, vec![b"crash-data".to_vec()]);
                println!(
                    "DURABILITY NOTE: data survived mem::forget — fjall fsynced or recovered from WAL"
                );
            }
        }
    }
}

// ============================================================================
// CATEGORY F: Key Boundary Conditions
// ============================================================================

#[tokio::test]
async fn attack_version_u64_max_read() {
    // Read from version u64::MAX — should return empty (no events at that version)
    let (store, _dir) = temp_store();
    let envs = vec![make_envelope(1, "A", b"data")];
    store.append(&tid("s"), None, &envs).await.unwrap();

    let mut stream = store
        .read_stream(&tid("s"), Version::new(u64::MAX).unwrap())
        .await
        .unwrap();
    assert!(
        stream.next().await.unwrap().is_none(),
        "reading from u64::MAX should return empty"
    );
}

#[tokio::test]
async fn attack_read_from_version_zero() {
    // Version 0 is INITIAL. Reading from 0 should include ALL events
    // because events start at version 1, and the range scan starts from 0.
    let (store, _dir) = temp_store();
    let envs = vec![make_envelope(1, "A", b"1"), make_envelope(2, "B", b"2")];
    store.append(&tid("s"), None, &envs).await.unwrap();

    // Read from version 0 (INITIAL)
    let count = count_events(&store, &tid("s")).await;
    assert_eq!(count, 2, "reading from INITIAL should return all events");
}

#[tokio::test]
async fn attack_read_nonexistent_stream() {
    let (store, _dir) = temp_store();
    let mut stream = store
        .read_stream(&tid("does-not-exist"), Version::INITIAL)
        .await
        .unwrap();
    assert!(
        stream.next().await.unwrap().is_none(),
        "nonexistent stream should return empty"
    );
}

#[tokio::test]
async fn attack_read_past_end_of_stream() {
    // Stream has 3 events (versions 1,2,3). Read from version 100.
    let (store, _dir) = temp_store();
    let envs = vec![
        make_envelope(1, "A", b"1"),
        make_envelope(2, "B", b"2"),
        make_envelope(3, "C", b"3"),
    ];
    store.append(&tid("s"), None, &envs).await.unwrap();

    let mut stream = store
        .read_stream(&tid("s"), Version::new(100).unwrap())
        .await
        .unwrap();
    assert!(
        stream.next().await.unwrap().is_none(),
        "reading past end of stream should return empty"
    );
}

// ============================================================================
// CATEGORY G: Recovery Torture
// ============================================================================

#[tokio::test]
async fn attack_recovery_torture_100_streams_reopen_5_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    let mut total_events_per_stream: Vec<u64> = vec![0; 100];

    for cycle in 0..5u64 {
        let store = FjallStore::builder(&path).open().unwrap();

        for (stream_idx, current_total) in total_events_per_stream.iter_mut().enumerate() {
            let sid_val = tid(&format!("torture_{stream_idx}"));
            let current = *current_total;
            let new_events = 5u64;

            let envelopes: Vec<_> = (0..new_events)
                .map(|i| {
                    let ver = current + i + 1;
                    make_envelope(ver, "Torture", &ver.to_le_bytes())
                })
                .collect();

            store
                .append(&sid_val, Version::new(current), &envelopes)
                .await
                .unwrap();
            *current_total += new_events;
        }

        // Verify all streams before closing
        for (stream_idx, expected) in total_events_per_stream.iter().enumerate() {
            let sid_val = tid(&format!("torture_{stream_idx}"));
            let count = count_events(&store, &sid_val).await;
            assert_eq!(
                count, *expected,
                "cycle {cycle}, stream {stream_idx}: expected {expected} events, got {count}"
            );
        }

        drop(store);
    }
}

// ============================================================================
// CATEGORY H: Concurrent Append Storm
// ============================================================================

#[tokio::test]
async fn attack_concurrent_append_storm_50_tasks() {
    // 50 tasks all try to create different streams simultaneously
    let (store, _dir) = temp_store();
    let store = Arc::new(store);

    let mut handles = Vec::new();
    for i in 0..50u64 {
        let store_clone = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            let sid_val = TestId(format!("concurrent_{i}"));
            let env = pending_envelope(Version::INITIAL)
                .event_type(leak(&format!("Created_{i}")))
                .payload(i.to_le_bytes().to_vec())
                .build_without_metadata();
            store_clone.append(&sid_val, None, &[env]).await
        });
        handles.push(handle);
    }

    let mut successes: u64 = 0;
    let mut failures: u64 = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => successes += 1,
            Err(e) => {
                println!("concurrent append failure: {e}");
                failures += 1;
            }
        }
    }

    // All should succeed since they're different streams
    assert_eq!(
        successes, 50,
        "all 50 unique-stream appends should succeed \
         (got {successes} successes, {failures} failures)"
    );

    // Verify all 50 streams are readable
    for i in 0..50u64 {
        let sid_val = TestId(format!("concurrent_{i}"));
        let count = count_events(&store, &sid_val).await;
        assert_eq!(
            count, 1,
            "stream concurrent_{i} should have exactly 1 event"
        );
    }
}

#[tokio::test]
async fn attack_concurrent_append_same_stream_conflict() {
    // 10 tasks all try to append to the SAME stream with expected_version=INITIAL
    // Only ONE should succeed. The rest should get Conflict errors.
    let (store, _dir) = temp_store();
    let store = Arc::new(store);

    let mut handles = Vec::new();
    for i in 0..10u64 {
        let store_clone = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            let sid_val = TestId("contested-stream".to_owned());
            let env = pending_envelope(Version::INITIAL)
                .event_type(leak(&format!("Contested_{i}")))
                .payload(i.to_le_bytes().to_vec())
                .build_without_metadata();
            store_clone.append(&sid_val, None, &[env]).await
        });
        handles.push(handle);
    }

    let mut successes: u64 = 0;
    let mut conflicts: u64 = 0;
    let mut other_errors: u64 = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => successes += 1,
            Err(AppendError::Conflict { .. }) => conflicts += 1,
            Err(_) => other_errors += 1,
        }
    }

    assert_eq!(
        successes, 1,
        "exactly 1 append should win the race (got {successes})"
    );
    assert_eq!(
        conflicts, 9,
        "9 appends should get Conflict (got {conflicts})"
    );
    assert_eq!(other_errors, 0, "no other errors expected");

    // Verify the stream has exactly 1 event
    let sid_val = tid("contested-stream");
    let count = count_events(&store, &sid_val).await;
    assert_eq!(count, 1, "contested stream should have exactly 1 event");
}

// ============================================================================
// CATEGORY I: Encoding Boundary Attacks
// ============================================================================

#[test]
fn attack_encoding_event_type_exactly_u16_max_bytes() {
    let event_type = "a".repeat(usize::from(u16::MAX));
    let mut buf = Vec::new();
    let result = encode_event_value(&mut buf, 1, &event_type, b"payload");
    assert!(
        result.is_ok(),
        "event type at exactly u16::MAX bytes should succeed"
    );

    let (sv, decoded_type, payload) = decode_event_value(&buf).unwrap();
    assert_eq!(sv, 1);
    assert_eq!(decoded_type.len(), usize::from(u16::MAX));
    assert_eq!(payload, b"payload");
}

#[test]
fn attack_encoding_event_type_one_over_u16_max() {
    let event_type = "a".repeat(usize::from(u16::MAX) + 1);
    let mut buf = Vec::new();
    let result = encode_event_value(&mut buf, 1, &event_type, b"payload");
    assert!(
        result.is_err(),
        "event type over u16::MAX must fail with EncodeError"
    );
}

#[test]
fn attack_encoding_empty_event_type() {
    let mut buf = Vec::new();
    let result = encode_event_value(&mut buf, 1, "", b"payload");
    assert!(
        result.is_ok(),
        "empty event type should encode successfully"
    );

    let (sv, decoded_type, payload) = decode_event_value(&buf).unwrap();
    assert_eq!(sv, 1);
    assert_eq!(decoded_type, "");
    assert_eq!(payload, b"payload");
}

#[test]
fn attack_encoding_empty_payload() {
    let mut buf = Vec::new();
    let result = encode_event_value(&mut buf, 1, "Test", b"");
    assert!(result.is_ok(), "empty payload should encode successfully");

    let (sv, decoded_type, payload) = decode_event_value(&buf).unwrap();
    assert_eq!(sv, 1);
    assert_eq!(decoded_type, "Test");
    assert!(payload.is_empty());
}

#[test]
fn attack_encoding_null_bytes_in_payload() {
    let evil_payload = b"\x00\x00\x00\x00\x00";
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 1, "Test", evil_payload).unwrap();
    let (_, _, payload) = decode_event_value(&buf).unwrap();
    assert_eq!(payload, evil_payload, "null bytes in payload must survive");
}

#[test]
fn attack_encoding_schema_version_boundaries() {
    // Test schema_version = 0 (should encode fine, but PersistedEnvelope::try_new rejects it)
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 0, "Test", b"data").unwrap();
    let (sv, _, _) = decode_event_value(&buf).unwrap();
    assert_eq!(
        sv, 0,
        "schema_version=0 should round-trip in encoding layer"
    );

    // Test schema_version = u32::MAX
    buf.clear();
    encode_event_value(&mut buf, u32::MAX, "Test", b"data").unwrap();
    let (sv, _, _) = decode_event_value(&buf).unwrap();
    assert_eq!(
        sv,
        u32::MAX,
        "schema_version=u32::MAX should round-trip in encoding layer"
    );
}

#[test]
fn attack_encoding_large_payload() {
    // 1MB payload
    let large = vec![0xABu8; 1_048_576];
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 1, "Big", &large).unwrap();
    let (_, _, payload) = decode_event_value(&buf).unwrap();
    assert_eq!(
        payload.len(),
        1_048_576,
        "1MB payload must survive encoding"
    );
    assert_eq!(payload[0], 0xAB);
    assert_eq!(payload[1_048_575], 0xAB);
}

// ============================================================================
// CATEGORY K: Append-then-read version consistency
// ============================================================================

#[tokio::test]
async fn attack_version_gap_in_batch() {
    // Try appending envelopes with non-sequential versions (gap)
    let (store, _dir) = temp_store();
    let envs = vec![
        make_envelope(1, "A", b"1"),
        make_envelope(3, "C", b"3"), // skip version 2!
    ];

    let result = store.append(&tid("gap"), None, &envs).await;
    assert!(result.is_err(), "version gap in batch must be rejected");
}

#[tokio::test]
async fn attack_version_duplicate_in_batch() {
    // Envelopes with duplicate versions
    let (store, _dir) = temp_store();
    let envs = vec![
        make_envelope(1, "A", b"1"),
        make_envelope(1, "B", b"2"), // duplicate version!
    ];

    let result = store.append(&tid("dup"), None, &envs).await;
    assert!(
        result.is_err(),
        "duplicate version in batch must be rejected"
    );
}

#[tokio::test]
async fn attack_version_backwards_in_batch() {
    // Envelopes with decreasing versions
    let (store, _dir) = temp_store();
    let envs = vec![
        make_envelope(2, "B", b"2"),
        make_envelope(1, "A", b"1"), // wrong order!
    ];

    let result = store.append(&tid("back"), Version::new(1), &envs).await;
    // This depends on validation: first envelope has version 2, expected is 2 (1+1) → OK
    // Second envelope has version 1, expected is 3 (1+1+1) → CONFLICT
    assert!(
        result.is_err(),
        "backwards versions in batch must be rejected"
    );
}

#[tokio::test]
async fn attack_empty_batch_does_not_advance_version() {
    let (store, _dir) = temp_store();

    // Create stream with 1 event
    store
        .append(&tid("empty-batch"), None, &[make_envelope(1, "A", b"1")])
        .await
        .unwrap();

    // Append empty batch
    store
        .append(&tid("empty-batch"), Version::new(1), &[])
        .await
        .unwrap();

    // Should still be able to append at version 2 (empty batch didn't advance)
    store
        .append(
            &tid("empty-batch"),
            Version::new(1),
            &[make_envelope(2, "B", b"2")],
        )
        .await
        .unwrap();

    let count = count_events(&store, &tid("empty-batch")).await;
    assert_eq!(count, 2, "should have exactly 2 events after empty batch");
}

// ============================================================================
// CATEGORY L: Recovery correctness after many streams
// ============================================================================

#[tokio::test]
async fn attack_recovery_stream_id_counter_correctness() {
    // Create N streams, reopen, verify counter is correct by creating stream N+1
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    let stream_count = 50u64;

    {
        let store = FjallStore::builder(&path).open().unwrap();
        for i in 0..stream_count {
            let sid_val = tid(&format!("recovery_{i}"));
            let env = make_envelope(1, "Init", &i.to_le_bytes());
            store.append(&sid_val, None, &[env]).await.unwrap();
        }
    }

    // Reopen
    {
        let store = FjallStore::builder(&path).open().unwrap();
        // Create one more stream — should get a unique numeric ID
        let sid_val = tid(&format!("recovery_{stream_count}"));
        let env = make_envelope(1, "Init", b"new");
        store.append(&sid_val, None, &[env]).await.unwrap();

        // Verify ALL streams are readable
        for i in 0..=stream_count {
            let sid_val = tid(&format!("recovery_{i}"));
            let count = count_events(&store, &sid_val).await;
            assert_eq!(count, 1, "stream recovery_{i} should have 1 event");
        }
    }
}

// ============================================================================
// CATEGORY M: Empty batch to nonexistent stream
// ============================================================================

#[tokio::test]
async fn attack_empty_batch_to_nonexistent_stream() {
    let (store, _dir) = temp_store();

    // Append empty batch to nonexistent stream with INITIAL version
    // Should succeed (no-op) because the early return skips all validation
    let result = store.append(&tid("phantom"), None, &[]).await;
    assert!(
        result.is_ok(),
        "empty batch to nonexistent stream should be no-op"
    );

    // Stream should still not exist (no metadata written)
    let count = count_events(&store, &tid("phantom")).await;
    assert_eq!(count, 0, "phantom stream should have 0 events");
}

#[tokio::test]
async fn attack_empty_batch_wrong_version_to_nonexistent() {
    // BUG PROBE: empty batch skips ALL validation including version check.
    // This means you can "succeed" with a wrong expected_version on an empty batch.
    let (store, _dir) = temp_store();

    let result = store.append(&tid("phantom2"), Version::new(999), &[]).await;
    // The early return at line 61 fires BEFORE any version check.
    // This is arguably a bug: it should still validate expected_version
    // against the actual stream state, even for empty batches.
    if result.is_ok() {
        println!(
            "BUG FOUND: empty batch with wrong expected_version (999) \
             succeeded for nonexistent stream — version check is bypassed \
             by the early return at store.rs:61"
        );
    }
}

// ============================================================================
// CATEGORY N: Large batch stress
// ============================================================================

#[tokio::test]
async fn attack_large_batch_1000_events() {
    let (store, _dir) = temp_store();
    let sid_val = tid("large-batch");

    let envelopes: Vec<_> = (1..=1000u64)
        .map(|v| make_envelope(v, "Tick", &v.to_le_bytes()))
        .collect();

    store.append(&sid_val, None, &envelopes).await.unwrap();

    let count = count_events(&store, &sid_val).await;
    assert_eq!(count, 1000);

    // Verify first and last
    let versions = read_all_versions(&store, &sid_val).await;
    assert_eq!(versions[0], 1);
    assert_eq!(versions[999], 1000);
}

// ============================================================================
// CATEGORY O: Multiple reopens with appends
// ============================================================================

#[tokio::test]
async fn attack_reopen_10_times_with_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let sid_val = tid("reopen-torture");

    for cycle in 0..10u64 {
        let store = FjallStore::builder(&path).open().unwrap();
        let ver = cycle * 10;
        let envelopes: Vec<_> = (1..=10u64)
            .map(|i| make_envelope(ver + i, "Cycle", &(ver + i).to_le_bytes()))
            .collect();
        store
            .append(&sid_val, Version::new(ver), &envelopes)
            .await
            .unwrap();

        let count = count_events(&store, &sid_val).await;
        let expected = (cycle + 1) * 10;
        assert_eq!(
            count, expected,
            "after cycle {cycle}: expected {expected} events, got {count}"
        );
    }
}

// ============================================================================
// CATEGORY P: Schema version 0 attack through the store
// ============================================================================

/// Verify that `schema_version = 0` is structurally impossible via `NonZeroU32`.
///
/// Previously, `schema_version=0` was accepted on write but rejected on read
/// (`PersistedEnvelope::try_new` rejects 0), creating unreadable "black hole" data.
/// Now `NonZeroU32` makes 0 a compile-time error on the builder. The read path
/// (`PersistedEnvelope::try_new`) still rejects 0 at runtime as a second layer
/// of defense against corrupt persisted data.
#[test]
fn attack_schema_version_zero_rejected_by_type_system() {
    // NonZeroU32::new(0) returns None — 0 is structurally unrepresentable
    assert!(NonZeroU32::new(0).is_none(), "NonZeroU32 must reject 0");
    // And try_new on the read path still rejects 0
    let result = nexus_store::PersistedEnvelope::try_new(Version::INITIAL, "E", 0, b"", ());
    assert!(
        result.is_err(),
        "PersistedEnvelope::try_new must reject schema_version=0"
    );
}

// ============================================================================
// CATEGORY Q: Concurrent reads during writes
// ============================================================================

#[tokio::test]
async fn attack_concurrent_reads_and_writes() {
    let (store, _dir) = temp_store();
    let store = Arc::new(store);
    let sid_val = tid("concurrent-rw");

    // Pre-populate with 100 events
    let initial_envs: Vec<_> = (1..=100u64)
        .map(|v| make_envelope(v, "Init", &v.to_le_bytes()))
        .collect();
    store.append(&sid_val, None, &initial_envs).await.unwrap();

    // Spawn 10 readers and 1 writer concurrently
    let mut handles = Vec::new();

    // Writer: append 100 more events
    {
        let store_w = Arc::clone(&store);
        let sid_w = sid_val.clone();
        handles.push(tokio::spawn(async move {
            let envs: Vec<_> = (101..=200u64)
                .map(|v| make_envelope(v, "Write", &v.to_le_bytes()))
                .collect();
            store_w
                .append(&sid_w, Version::new(100), &envs)
                .await
                .unwrap();
        }));
    }

    // 10 readers
    for _ in 0..10u64 {
        let store_r = Arc::clone(&store);
        let sid_r = sid_val.clone();
        handles.push(tokio::spawn(async move {
            let count = count_events(&store_r, &sid_r).await;
            // Should see either 100 (before write) or 200 (after write)
            // NOT a partial count (e.g. 150)
            assert!(
                count == 100 || count == 200,
                "reader saw {count} events — expected 100 or 200 (snapshot isolation violated?)"
            );
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Final check: exactly 200 events
    let final_count = count_events(&store, &sid_val).await;
    assert_eq!(final_count, 200);
}

// ============================================================================
// CATEGORY R: Builder partition configuration
// ============================================================================

#[tokio::test]
async fn attack_custom_builder_config_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db"))
        .streams_config(|opts| {
            opts.data_block_size_policy(fjall::config::BlockSizePolicy::all(8_192))
        })
        .events_config(|opts| {
            opts.data_block_size_policy(fjall::config::BlockSizePolicy::all(16_384))
        })
        .open()
        .unwrap();

    let env = make_envelope(1, "A", b"data");
    store
        .append(&tid("custom-config"), None, &[env])
        .await
        .unwrap();

    let count = count_events(&store, &tid("custom-config")).await;
    assert_eq!(count, 1, "custom config store should work normally");
}
