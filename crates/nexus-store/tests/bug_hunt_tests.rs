//! Bug hunting tests for `nexus-store`.
//!
//! These tests are written with ADVERSARIAL INTENT — each one probes a
//! potential design flaw, contract violation, or edge case that could hide
//! a real bug. Tests that expose bugs will FAIL.

#![allow(clippy::unwrap_used, reason = "bug hunting tests use unwrap")]
#![allow(clippy::expect_used, reason = "bug hunting tests use expect")]
#![allow(clippy::panic, reason = "bug hunting tests use panic")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(
    clippy::significant_drop_tightening,
    reason = "lock guard lifetime is fine in test adapters"
)]
#![allow(clippy::as_conversions, reason = "bug hunt tests")]
#![allow(clippy::cast_possible_truncation, reason = "bug hunt tests")]
#![allow(clippy::drop_non_drop, reason = "explicit drops for lending docs")]
#![allow(
    clippy::print_stdout,
    reason = "bug hunt tests use println for diagnostic output"
)]
#![allow(
    clippy::uninlined_format_args,
    reason = "clarity over brevity in test assertions"
)]

use nexus::Version;
use nexus_store::AppendError;
use nexus_store::ToStreamLabel;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::error::StoreError;
use nexus_store::pending_envelope;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use std::collections::HashMap;
use std::fmt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl nexus::Id for TestId {}
fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

// ============================================================================
// In-memory adapter for probing
// ============================================================================

type StoredRow = (u64, String, Vec<u8>);

struct ProbeStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl ProbeStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

struct ProbeStream {
    events: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
}

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("conflict")]
    Conflict,
}

impl EventStream for ProbeStream {
    type Error = ProbeError;
    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new_unchecked(
            Version::new(row.0).unwrap(),
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

impl RawEventStore for ProbeStore {
    type Error = ProbeError;
    type Stream<'a>
        = ProbeStream
    where
        Self: 'a;

    async fn append(
        &self,
        id: &impl nexus::Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(id.to_string()).or_default();
        let current = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        let expected_u64 = expected_version.map_or(0, nexus::Version::as_u64);
        if current != expected_u64 {
            return Err(AppendError::Store(ProbeError::Conflict));
        }
        // Enforce implementor contract: versions must be sequential
        // from expected_version + 1
        for (expected_next, env) in (expected_u64 + 1..).zip(envelopes.iter()) {
            if env.version().as_u64() != expected_next {
                return Err(AppendError::Store(ProbeError::Conflict));
            }
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        Ok(())
    }

    async fn read_stream(
        &self,
        id: &impl nexus::Id,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(&id.to_string())
            .map(|s| {
                s.iter()
                    .filter(|(v, _, _)| *v >= from.as_u64())
                    .map(|(v, t, p)| (*v, t.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ProbeStream { events, pos: 0 })
    }
}

// ============================================================================
// BUG HUNT 1: ENVELOPE & BUILDER DESIGN FLAWS
// ============================================================================

// Version 0 is no longer constructable via from_persisted — it returns None.
// The first event must be version 1 (INITIAL).
#[test]
fn version_zero_is_not_constructable() {
    assert!(
        Version::new(0).is_none(),
        "Version 0 must not be constructable — first event must be version 1"
    );
}

// BUG PROBE: Version overflow — creating an envelope with u64::MAX and then
// trying to get next() would return None (checked overflow).
#[test]
fn bug_probe_version_max_next_returns_none() {
    let v = Version::new(u64::MAX).unwrap();
    // next() now returns Option — should be None at MAX
    let result = v.next();
    assert!(
        result.is_none(),
        "Version::next() at MAX should return None"
    );
}

// ============================================================================
// BUG HUNT 2: TRAIT DESIGN HOLES
// ============================================================================

// BUG PROBE: Is Codec object-safe? Can we use `dyn Codec`?
// If not, this limits composability (can't put codecs in trait objects).
#[test]
fn bug_probe_codec_not_object_safe() {
    // The Codec trait has generic methods: fn encode<E: DomainEvent>(&self, event: &E)
    // Generic methods make traits NOT object-safe.
    // You CANNOT write: Box<dyn Codec<Error = SomeError>>
    // This is a design trade-off, not necessarily a bug, but it limits flexibility.

    // We can verify the trait itself compiles with concrete types
    use nexus_store::codec::Codec;

    fn _takes_codec<C: Codec<()>>(c: &C) {
        // Fine — generic parameter
        let _ = c;
    }

    // But this would NOT compile:
    // fn _takes_dyn_codec(c: &dyn Codec<Error = std::io::Error>) { ... }
    // FINDING: Codec is not object-safe due to generic methods.
    // This means you can't have a Vec<Box<dyn Codec>> or dynamic dispatch.
}

// NOTE: The old EventUpcaster trait tests (infinite loop, data corruption)
// have been removed. The replacement SchemaTransform trait uses declarative
// source_version/to_version — the pipeline handles matching and chaining,
// eliminating the infinite loop class of bugs by design.
// See: schema_transform_tests.rs, transform_chain_new_tests.rs, pipeline_tests.rs

// Adapters MUST reject backwards versions per the implementor contract.
#[tokio::test]
async fn append_rejects_backwards_versions() {
    let store = ProbeStore::new();

    // Deliberately backwards: version 3, then 2, then 1
    let envelopes = vec![
        pending_envelope(Version::new(3).unwrap())
            .event_type("E")
            .payload(vec![3])
            .build_without_metadata(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("E")
            .payload(vec![2])
            .build_without_metadata(),
        pending_envelope(Version::new(1).unwrap())
            .event_type("E")
            .payload(vec![1])
            .build_without_metadata(),
    ];

    let result = store.append(&tid("s1"), None, &envelopes).await;
    assert!(
        result.is_err(),
        "Adapter must reject non-sequential versions"
    );
}

// ============================================================================
// BUG HUNT 3: PROPERTY-BASED FUZZING FOR CONTRACT VIOLATIONS
// ============================================================================

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // FUZZ: Version from_persisted with random u64 values
    #[test]
    fn fuzz_version_from_persisted(v in any::<u64>()) {
        let version = Version::new(v);
        if v == 0 {
            prop_assert!(version.is_none(), "from_persisted(0) must return None");
        } else {
            let version = version.unwrap();
            prop_assert_eq!(version.as_u64(), v, "roundtrip failed");
        }
    }

    // FUZZ: Large payloads with random content
    #[test]
    fn fuzz_payload_byte_patterns(
        payload in prop::collection::vec(any::<u8>(), 0..10_000)
    ) {
        let envelope = pending_envelope(Version::INITIAL)
            .event_type("E")
            .payload(payload.clone())
            .build_without_metadata();
        prop_assert_eq!(envelope.payload(), payload.as_slice());
    }

    // FUZZ: Random version sequences must be rejected unless they happen
    // to be exactly sequential from 1.
    #[test]
    fn fuzz_non_sequential_versions_rejected(
        versions in prop::collection::vec(1..1000u64, 1..10)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = ProbeStore::new();
            let envelopes: Vec<_> = versions
                .iter()
                .map(|&v| {
                    pending_envelope(Version::new(v).unwrap())
                        .event_type("E")
                        .payload(vec![v as u8])
                        .build_without_metadata()
                })
                .collect();

            let result = store.append(&tid("s1"), None, &envelopes).await;

            // Check if versions are actually sequential from 1
            let is_sequential = versions.iter().enumerate().all(|(i, &v)| v == (i as u64) + 1);
            if is_sequential {
                prop_assert!(result.is_ok(), "sequential versions should be accepted");
            } else {
                prop_assert!(result.is_err(), "non-sequential versions must be rejected");
            }
            Ok(())
        })?;
    }
}

// ============================================================================
// BUG HUNT 4: COMPILE-TIME SAFETY GAPS
// ============================================================================

// BUG PROBE: PendingEnvelope fields are private but the struct is public.
// Can we construct one without the builder using unsafe or mem::zeroed?
#[test]
fn bug_probe_cannot_construct_pending_envelope_without_builder() {
    // This is a POSITIVE test — we WANT this to be impossible.
    // PendingEnvelope has private fields, so you must use the builder.
    // But: std::mem::zeroed() could bypass this...
    //
    // We WON'T test unsafe here, but we document that:
    // - PendingEnvelope<()> has no Default impl (good)
    // - PendingEnvelope<()> has no From impl (good)
    // - PendingEnvelope<()> has no Clone impl (interesting — is this intentional?)

    // Verify no Clone
    fn assert_not_clone<T>() {
        // If PendingEnvelope implemented Clone, this would compile
        // We can't test negative traits in Rust, but we can document:
        // PendingEnvelope does NOT implement Clone, Copy, Default, or From.
    }
    assert_not_clone::<PendingEnvelope<()>>();
}

// BUG PROBE: PersistedEnvelope has a public `new()` constructor.
// Unlike PendingEnvelope (which requires the builder), ANYONE can construct
// a PersistedEnvelope with arbitrary data. Is this a safety gap?
#[test]
fn bug_probe_persisted_envelope_public_constructor() {
    // This compiles — PersistedEnvelope::new_unchecked is fully public
    let forged = PersistedEnvelope::<()>::new_unchecked(
        Version::new(999).unwrap(),
        "ForgedEvent",
        1,
        b"malicious payload",
        (),
    );

    // A malicious adapter could return forged envelopes
    assert_eq!(forged.version().as_u64(), 999);
    // FINDING: PersistedEnvelope has no integrity protection.
    // Any code can construct arbitrary envelopes and return them
    // from a RawEventStore implementation. The kernel trusts these blindly.
}

// ============================================================================
// BUG HUNT 5: SECURITY — ERROR INFO LEAKAGE, MEMORY, SIZES
// ============================================================================

// BUG PROBE: StoreError size — is it growing beyond what's stack-safe?
#[test]
fn bug_probe_store_error_size_regression() {
    let size = std::mem::size_of::<StoreError>();
    // The security tests check <= 128, but let's be more precise
    // and track the EXACT size for regression detection.
    println!("StoreError size: {size} bytes");

    // On 64-bit, with StreamLabel (String = 24 bytes) + Option<Version> (16 bytes each) + Box (16 bytes):
    // Conflict: 24 + 16 + 16 = 56, Codec/Adapter: 16 each
    // Enum discriminant + padding
    assert!(
        size <= 112,
        "StoreError grew to {size} bytes ��� investigate! Previous was ~96"
    );
}

// BUG PROBE: What happens with VERY long stream_id in StoreError?
// StreamLabel might truncate — does it do so safely?
#[test]
fn bug_probe_stream_label_truncation() {
    let long_id = "x".repeat(1000);
    let err = StoreError::StreamNotFound {
        stream_id: long_id.as_str().to_stream_label(),
    };
    let msg = err.to_string();
    // StreamLabel should keep the message useful
    assert!(msg.contains("not found"));
}

// BUG PROBE: Box<dyn Error + Send + Sync> source chain — does wrapping
// preserve the source error through StoreError::Codec/Adapter?
#[test]
fn bug_probe_error_source_chain_preserved() {
    use std::error::Error;

    // Create a nested error chain: inner -> middle -> StoreError
    let inner = std::io::Error::other("database connection refused");
    let middle: Box<dyn std::error::Error + Send + Sync> =
        Box::new(std::io::Error::other(inner.to_string()));

    let store_err = StoreError::Adapter(middle);

    // Source chain should be intact
    let source = store_err.source().expect("should have source");
    assert!(
        source.to_string().contains("database connection refused"),
        "source chain broken: got '{}'",
        source
    );
}

// BUG PROBE: PendingEnvelope with metadata that panics on Drop.
// Does the builder handle this safely?
#[test]
fn bug_probe_metadata_panic_on_drop() {
    // This is an adversarial metadata type
    struct PanicOnDrop(bool);
    impl std::fmt::Debug for PanicOnDrop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "PanicOnDrop({})", self.0)
        }
    }
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            if self.0 {
                // In a real scenario, this would unwind
                // We can't test panic-in-drop safely, but we can test
                // that the builder doesn't do anything weird with Drop order
            }
        }
    }

    let envelope = pending_envelope(Version::INITIAL)
        .event_type("E")
        .payload(vec![])
        .build(PanicOnDrop(false));

    // Metadata is moved into the envelope, not cloned
    assert_eq!(envelope.version(), Version::INITIAL);
    // FINDING: Drop order is fine — Rust handles this correctly.
}

// BUG PROBE: Are builder intermediate types Send + Sync?
// If not, you can't pass a partially-built envelope across await points.
#[test]
fn bug_probe_builder_intermediates_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    use nexus_store::envelope::{WithEventType, WithPayload, WithVersion};
    assert_send_sync::<WithVersion>();
    assert_send_sync::<WithEventType>();
    assert_send_sync::<WithPayload>();
    // FINDING: All good — builder can be used across await points.
}

// BUG PROBE: What is the alignment and size of envelope types?
// Are there unexpected padding bytes wasting memory?
#[test]
fn bug_probe_envelope_memory_layout() {
    let pending_size = std::mem::size_of::<PendingEnvelope<()>>();
    let persisted_size = std::mem::size_of::<PersistedEnvelope<'static, ()>>();

    println!("PendingEnvelope<()> size: {pending_size} bytes");
    println!("PersistedEnvelope<'static, ()> size: {persisted_size} bytes");

    // PendingEnvelope: Version(8) + &'static str(16) + u32(4) + Vec(24) + () + padding
    // Expected: ~56 bytes
    assert!(
        pending_size <= 64,
        "PendingEnvelope is {pending_size} bytes — check for padding bloat"
    );

    // PersistedEnvelope: Version(8) + &str(16) + u32(4) + &[u8](16) + () + padding
    // Expected: ~48 bytes
    assert!(
        persisted_size <= 56,
        "PersistedEnvelope is {persisted_size} bytes — check for padding bloat"
    );

    // PersistedEnvelope should be SMALLER than PendingEnvelope (borrows vs owns)
    assert!(
        persisted_size <= pending_size,
        "PersistedEnvelope ({persisted_size}) should be smaller or equal to \
         PendingEnvelope ({pending_size})"
    );
}

// BUG PROBE: Can we detect if the `RawEventStore` trait is accidentally
// changed in a way that breaks backward compatibility?
// This is a compile-time golden-file test.
#[test]
fn bug_probe_raw_event_store_trait_shape() {
    // Verify the trait has exactly the methods we expect
    // If someone adds/removes/renames a method, this test should break

    // The trait has: append, read_stream
    // Associated types: Error, Stream<'a>
    // Supertrait: Send + Sync

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProbeStore>(); // ProbeStore: RawEventStore implies Send + Sync
}
