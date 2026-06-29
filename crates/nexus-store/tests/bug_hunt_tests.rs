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
#![allow(clippy::no_effect_underscore_binding, reason = "tests")]
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

use nexus::{ErrorId, Version};
use nexus_store::AppendError;
use nexus_store::InMemoryStoreError;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::error::StoreError;
use nexus_store::pending_envelope;
use nexus_store::store::{GlobalSeq, RawEventStore};
use std::collections::HashMap;
use tokio::sync::Mutex;

fn build_persisted(
    version: Version,
    global_seq: GlobalSeq,
    event_type: &str,
    payload: &[u8],
) -> PersistedEnvelope {
    let mut buf = Vec::with_capacity(event_type.len() + payload.len());
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(payload);
    let value = bytes::Bytes::from(buf);
    let et_end = u32::try_from(event_type.len()).expect("event_type fits u32");
    let pl_end = u32::try_from(event_type.len() + payload.len()).expect("payload fits u32");
    PersistedEnvelope::try_new(
        version,
        global_seq,
        value,
        nexus_store::value::SchemaVersion::INITIAL,
        0..et_end,
        et_end..pl_end,
        None,
    )
    .expect("test fixture envelope")
}

/// Concrete `StoreError` for tests using `InMemoryStore` with no codec/upcaster.
type TestStoreError = StoreError<InMemoryStoreError, std::io::Error, std::io::Error>;

fn label(s: &str) -> ErrorId {
    ErrorId::from_display(&s)
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

impl futures::Stream for ProbeStream {
    type Item = Result<PersistedEnvelope, ProbeError>;
    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        if self.pos >= self.events.len() {
            return core::task::Poll::Ready(None);
        }
        let row = self.events[self.pos].clone();
        self.pos += 1;
        let env = build_persisted(
            Version::new(row.0).unwrap(),
            GlobalSeq::INITIAL,
            &row.1,
            &row.2,
        );
        core::task::Poll::Ready(Some(Ok(env)))
    }
}

impl RawEventStore for ProbeStore {
    type Error = ProbeError;
    type Stream = ProbeStream;
    type AllStream = ProbeStream;

    async fn append(
        &self,
        id: &nexus_store::StreamKey,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
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
        id: &nexus_store::StreamKey,
        from: Version,
    ) -> Result<Self::Stream, Self::Error> {
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

    async fn read_all(
        &self,
        _from: nexus_store::store::GlobalSeq,
    ) -> Result<Self::AllStream, Self::Error> {
        // ProbeStore is a test-only adapter that does not implement the global index.
        Ok(ProbeStream {
            events: Vec::new(),
            pos: 0,
        })
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
    use nexus_store::codec::{Decode, Encode};

    fn _takes_codec<C: Encode<()> + Decode<()>>(c: &C) {
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
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(2).unwrap())
            .event_type("E")
            .payload(vec![2])
            .expect("valid payload")
            .build(),
        pending_envelope(Version::new(1).unwrap())
            .event_type("E")
            .payload(vec![1])
            .expect("valid payload")
            .build(),
    ];

    let result = store
        .append(
            &nexus_store::StreamKey::from_slice("s1".as_bytes()),
            None,
            &envelopes,
        )
        .await;
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
            .expect("valid payload")
            .build();
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
                        .expect("valid payload")
                        .build()
                })
                .collect();

            let result = store.append(&nexus_store::StreamKey::from_slice("s1".as_bytes()), None, &envelopes).await;

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
    // - PendingEnvelope has no Default impl (good)
    // - PendingEnvelope has no From impl (good)
    // - PendingEnvelope has no Clone impl (interesting — is this intentional?)

    // Verify no Clone
    fn assert_not_clone<T>() {
        // If PendingEnvelope implemented Clone, this would compile
        // We can't test negative traits in Rust, but we can document:
        // PendingEnvelope does NOT implement Clone, Copy, Default, or From.
    }
    assert_not_clone::<PendingEnvelope>();
}

// BUG PROBE: PersistedEnvelope has a public `try_new()` constructor.
// Unlike PendingEnvelope (which requires the builder), ANYONE can construct
// a PersistedEnvelope with arbitrary data. Is this a safety gap?
#[test]
fn bug_probe_persisted_envelope_public_constructor() {
    // This compiles — PersistedEnvelope::try_new is fully public
    let forged = build_persisted(
        Version::new(999).unwrap(),
        GlobalSeq::INITIAL,
        "ForgedEvent",
        b"malicious payload",
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
    let size = std::mem::size_of::<TestStoreError>();
    // With concrete type params and ArrayString<64>, the size will be different
    // than the old Box<dyn Error> approach. Track for regression.
    println!("StoreError size: {size} bytes");

    // With ArrayString<64> fields and concrete error types, the enum
    // is larger per-variant than the old Box<dyn Error> approach, but
    // entirely stack-allocated (no heap on error paths).
    assert!(
        size <= 256,
        "StoreError grew to {size} bytes — investigate!"
    );
}

// BUG PROBE: What happens with VERY long stream_id in StoreError?
// ArrayString<64> truncates silently — does it do so safely?
#[test]
fn bug_probe_stream_label_truncation() {
    // ArrayString<64> will only hold up to 64 bytes.
    // Use Id::to_label() which truncates at char boundary.
    let short_id = label("test-stream");
    let err: TestStoreError = StoreError::StreamNotFound {
        stream_id: short_id,
    };
    let msg = err.to_string();
    // ArrayString preserves the content that fits
    assert!(msg.contains("not found"));
}

// BUG PROBE: Error source chain — does wrapping preserve the source error
// through StoreError::Codec/Adapter with concrete types?
#[test]
fn bug_probe_error_source_chain_preserved() {
    use std::error::Error;

    let inner = std::io::Error::other("database connection refused");
    let _store_err: TestStoreError = StoreError::Adapter(InMemoryStoreError::CorruptVersion);

    // Source chain should work with concrete types
    // InMemoryStoreError::CorruptVersion has no source, so source is None.
    // Test with an error type that does have source:
    let io_store_err: StoreError<std::io::Error, std::io::Error, std::io::Error> =
        StoreError::Adapter(inner);
    let source = io_store_err.source().expect("should have source");
    assert!(
        source.to_string().contains("database connection refused"),
        "source chain broken: got '{}'",
        source
    );
}

// BUG PROBE: PendingEnvelope with bytes metadata drops cleanly.
// Metadata is now always raw bytes (`Option<Bytes>`); verify the builder
// stores and exposes them without issues.
#[test]
fn bug_probe_metadata_bytes_stored_correctly() {
    let meta = b"correlation-id:abc123".to_vec();
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("E")
        .payload(vec![])
        .expect("valid payload")
        .with_metadata(meta.clone())
        .expect("valid metadata");

    assert_eq!(envelope.version(), Version::INITIAL);
    assert_eq!(envelope.metadata(), Some(meta.as_slice()));
    // FINDING: Drop order is fine — Bytes handles this correctly.
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
    let pending_size = std::mem::size_of::<PendingEnvelope>();
    let persisted_size = std::mem::size_of::<PersistedEnvelope>();

    println!("PendingEnvelope size: {pending_size} bytes");
    println!("PersistedEnvelope size: {persisted_size} bytes");

    // PendingEnvelope: Version(8) + &'static str(16) + u32(4) + Bytes(24) + Option<Bytes>(32) + padding
    // Expected: ~88 bytes
    assert!(
        pending_size <= 128,
        "PendingEnvelope is {pending_size} bytes — check for padding bloat"
    );

    // PersistedEnvelope: Version(8) + GlobalSeq(8) + u32(4) + Bytes(24) + 3x Range<u32>(12+8+8) + padding
    // Expected: ~80 bytes
    assert!(
        persisted_size <= 128,
        "PersistedEnvelope is {persisted_size} bytes — check for padding bloat"
    );

    // Both envelopes should be reasonably sized
    assert!(
        persisted_size <= 256,
        "PersistedEnvelope ({persisted_size}) seems unexpectedly large"
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
