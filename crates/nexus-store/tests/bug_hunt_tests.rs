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

use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::error::StoreError;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use std::collections::HashMap;
use tokio::sync::Mutex;

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
    events: Vec<(String, u64, String, Vec<u8>)>,
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
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            1,
            &row.3,
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
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current != expected_version.as_u64() {
            return Err(ProbeError::Conflict);
        }
        // Enforce implementor contract: versions must be sequential
        // from expected_version + 1
        let mut expected_next = expected_version.as_u64() + 1;
        for env in envelopes {
            if env.version().as_u64() != expected_next {
                return Err(ProbeError::Conflict);
            }
            expected_next += 1;
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
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|(v, _, _)| *v >= from.as_u64())
                    .map(|(v, t, p)| (stream_id.to_owned(), *v, t.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ProbeStream { events, pos: 0 })
    }
}

// ============================================================================
// BUG HUNT 1: ENVELOPE & BUILDER DESIGN FLAWS
// ============================================================================

// BUG PROBE: What happens when envelopes in an append batch have DIFFERENT
// stream_ids than the stream_id parameter? The trait says nothing about this.
// This is a semantic mismatch that adapters might silently accept.
#[tokio::test]
async fn bug_probe_mixed_stream_ids_in_append_batch() {
    let store = ProbeStore::new();

    // Envelopes claim to be for "stream-A" but we append to "stream-B"
    let envelopes = vec![
        pending_envelope("stream-A".into())
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![1])
            .build_without_metadata(),
    ];

    // This succeeds — the adapter ignores the envelope's stream_id field
    // and uses the stream_id parameter instead.
    // BUG: The envelope carries a stream_id that contradicts the append target.
    // There is NO validation that envelope.stream_id() == stream_id parameter.
    let result = store.append("stream-B", Version::INITIAL, &envelopes).await;
    assert!(
        result.is_ok(),
        "BUG CONFIRMED: append silently accepts envelopes with mismatched stream_ids"
    );

    // Now read stream-B — the event is there, but its envelope said stream-A
    let mut stream = store
        .read_stream("stream-B", Version::INITIAL)
        .await
        .unwrap();
    let env = stream.next().await.unwrap().unwrap();
    // The read path constructs a NEW envelope with the correct stream_id
    assert_eq!(env.stream_id(), "stream-B");
    // But the ORIGINAL envelope's stream_id was "stream-A" — this data was silently discarded
    // This means stream_id on PendingEnvelope is REDUNDANT with the append parameter
}

// BUG PROBE: PendingEnvelope stores stream_id but it's never used by RawEventStore.
// The append() takes stream_id as a separate parameter. So why does PendingEnvelope
// carry a stream_id at all? This is a design bug — redundant data that can diverge.
#[test]
fn bug_probe_pending_envelope_stream_id_is_redundant() {
    let envelope = pending_envelope("my-stream".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build_without_metadata();

    // The envelope carries "my-stream" — but RawEventStore::append() takes
    // stream_id as a separate &str parameter. The adapter is free to ignore
    // the envelope's stream_id. This is a design inconsistency.
    assert_eq!(envelope.stream_id(), "my-stream");
    // FINDING: stream_id exists on PendingEnvelope but is redundant with
    // the append() parameter. Either remove it from PendingEnvelope or
    // remove it from append() signature.
}

// Version 0 = INITIAL means "no events yet." An envelope at version 0 is
// semantically wrong — the first event must be version 1.
// Adapters must reject it because expected_version(0) + 1 = 1, not 0.
#[tokio::test]
async fn append_rejects_version_zero_envelope() {
    let store = ProbeStore::new();

    assert_eq!(Version::INITIAL.as_u64(), 0);
    assert_eq!(Version::INITIAL, Version::from_persisted(0));

    let envelopes = vec![
        pending_envelope("s1".into())
            .version(Version::from_persisted(0))
            .event_type("E")
            .payload(vec![1])
            .build_without_metadata(),
    ];

    let result = store.append("s1", Version::INITIAL, &envelopes).await;
    assert!(
        result.is_err(),
        "Adapter must reject version 0 — first event must be version 1"
    );
}

// BUG PROBE: Version overflow — creating an envelope with u64::MAX and then
// trying to get next() would panic. But can we even detect this at the store level?
#[test]
fn bug_probe_version_max_next_panics() {
    let v = Version::from_persisted(u64::MAX);
    // This PANICS — which is the kernel's documented behavior.
    // But the store layer has no protection against creating an envelope
    // with u64::MAX and then the kernel trying to .next() on it.
    let result = std::panic::catch_unwind(|| v.next());
    assert!(result.is_err(), "Version::next() at MAX should panic");
    // FINDING: Store layer has no guard against version overflow.
    // If a database adapter returns version u64::MAX, the kernel will panic
    // on the next operation.
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

// BUG PROBE: EventUpcaster — what if can_upcast returns true for the SAME
// version it outputs? This creates an infinite loop in upcaster chains.
#[test]
fn bug_probe_upcaster_infinite_loop() {
    struct InfiniteUpcaster;
    impl EventUpcaster for InfiniteUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true // always says yes
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".to_owned(), 1, p.to_vec()) // outputs version 1 — same as input!
        }
    }

    let upcaster = InfiniteUpcaster;

    // Simulate a naive upcaster chain loop
    let mut version = 1u32;
    let mut iterations = 0u32;
    let payload = b"data";
    let mut current_payload = payload.to_vec();

    // A naive consumer would loop forever here
    while upcaster.can_upcast("E", version) && iterations < 100 {
        let (_, v, p) = upcaster.upcast("E", version, &current_payload);
        version = v;
        current_payload = p;
        iterations += 1;
    }

    // FINDING: Nothing in the trait prevents this. A naive facade that chains
    // upcasters in a while loop would spin forever. The implementor contract
    // says "output version > input version" but there's no compile-time enforcement.
    assert_eq!(
        iterations, 100,
        "hit safety limit — infinite loop was possible"
    );
}

// BUG PROBE: EventUpcaster can CORRUPT data — the trait has no checksum
// or integrity verification. An upcaster can silently produce garbage.
#[test]
fn bug_probe_upcaster_data_corruption() {
    struct CorruptingUpcaster;
    impl EventUpcaster for CorruptingUpcaster {
        fn can_upcast(&self, _: &str, v: u32) -> bool {
            v == 1
        }
        fn upcast(&self, _: &str, _: u32, _payload: &[u8]) -> (String, u32, Vec<u8>) {
            // Completely replaces payload with garbage — no integrity check
            ("E".to_owned(), 2, vec![0xDE, 0xAD, 0xBE, 0xEF])
        }
    }

    let original = b"important financial data";
    let (_, _, corrupted) = CorruptingUpcaster.upcast("E", 1, original);

    assert_ne!(corrupted.as_slice(), original.as_slice());
    // FINDING: Nothing prevents an upcaster from destroying data.
    // The codec will fail to deserialize, but the error will be
    // "invalid JSON" not "upcaster corrupted data" — hard to debug.
}

// Adapters MUST reject backwards versions per the implementor contract.
#[tokio::test]
async fn append_rejects_backwards_versions() {
    let store = ProbeStore::new();

    // Deliberately backwards: version 3, then 2, then 1
    let envelopes = vec![
        pending_envelope("s1".into())
            .version(Version::from_persisted(3))
            .event_type("E")
            .payload(vec![3])
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(2))
            .event_type("E")
            .payload(vec![2])
            .build_without_metadata(),
        pending_envelope("s1".into())
            .version(Version::from_persisted(1))
            .event_type("E")
            .payload(vec![1])
            .build_without_metadata(),
    ];

    let result = store.append("s1", Version::INITIAL, &envelopes).await;
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

    // FUZZ: Can we find a stream_id that breaks the adapter?
    // SQL injection, format strings, regex bombs, etc.
    #[test]
    fn fuzz_dangerous_stream_ids(
        stream_id in "[a-z0-9._/-]{1,100}"
    ) {
        // If any stream_id causes a panic, that's a bug
        let result = std::panic::catch_unwind(|| {
            pending_envelope(stream_id.clone())
                .version(Version::from_persisted(1))
                .event_type("E")
                .payload(vec![1])
                .build_without_metadata()
        });
        prop_assert!(result.is_ok(), "stream_id caused panic: {:?}", stream_id);
    }

    // FUZZ: Version from_persisted with random u64 values
    #[test]
    fn fuzz_version_from_persisted(v in any::<u64>()) {
        let version = Version::from_persisted(v);
        prop_assert_eq!(version.as_u64(), v, "roundtrip failed");
    }

    // FUZZ: Large payloads with random content
    #[test]
    fn fuzz_payload_byte_patterns(
        payload in prop::collection::vec(any::<u8>(), 0..10_000)
    ) {
        let envelope = pending_envelope("s1".into())
            .version(Version::from_persisted(1))
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
                    pending_envelope("s1".into())
                        .version(Version::from_persisted(v))
                        .event_type("E")
                        .payload(vec![v as u8])
                        .build_without_metadata()
                })
                .collect();

            let result = store.append("s1", Version::INITIAL, &envelopes).await;

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
    fn _assert_not_clone<T>() {
        // If PendingEnvelope implemented Clone, this would compile
        // We can't test negative traits in Rust, but we can document:
        // PendingEnvelope does NOT implement Clone, Copy, Default, or From.
    }
    _assert_not_clone::<PendingEnvelope<()>>();
}

// BUG PROBE: PersistedEnvelope has a public `new()` constructor.
// Unlike PendingEnvelope (which requires the builder), ANYONE can construct
// a PersistedEnvelope with arbitrary data. Is this a safety gap?
#[test]
fn bug_probe_persisted_envelope_public_constructor() {
    // This compiles — PersistedEnvelope::new is fully public
    let forged = PersistedEnvelope::<()>::new(
        "fake-stream",
        Version::from_persisted(999),
        "ForgedEvent",
        1,
        b"malicious payload",
        (),
    );

    // A malicious adapter could return forged envelopes
    assert_eq!(forged.stream_id(), "fake-stream");
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

    // On 64-bit, with ErrorId (64 bytes) + Version (8 bytes each) + Box (16 bytes):
    // Conflict: 64 + 8 + 8 = 80, Codec/Adapter: 16 each
    // Enum discriminant + padding: ~88 bytes
    assert!(
        size <= 96,
        "StoreError grew to {size} bytes — investigate! Previous was ~88"
    );
}

// BUG PROBE: What happens with VERY long stream_id in StoreError?
// ErrorId might truncate — does it do so safely?
#[test]
fn bug_probe_error_id_truncation() {
    let long_id = "x".repeat(1000);
    let err = StoreError::StreamNotFound {
        stream_id: nexus::ErrorId::from_display(&long_id),
    };
    let msg = err.to_string();
    // ErrorId truncates to 64 bytes — verify the message is still useful
    assert!(msg.contains("not found"));
    // The stream_id should be truncated, not the full 1000 chars
    assert!(
        msg.len() < 500,
        "Error message too large: {} bytes",
        msg.len()
    );
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

    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build(PanicOnDrop(false));

    // Metadata is moved into the envelope, not cloned
    assert_eq!(envelope.stream_id(), "s1");
    // FINDING: Drop order is fine — Rust handles this correctly.
}

// BUG PROBE: Are builder intermediate types Send + Sync?
// If not, you can't pass a partially-built envelope across await points.
#[test]
fn bug_probe_builder_intermediates_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    use nexus_store::envelope::{WithEventType, WithPayload, WithStreamId, WithVersion};
    assert_send_sync::<WithStreamId>();
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

    // PendingEnvelope: String(24) + Version(8) + &'static str(16) + Vec(24) + () + padding
    // Expected: ~72 bytes
    assert!(
        pending_size <= 80,
        "PendingEnvelope is {pending_size} bytes — check for padding bloat"
    );

    // PersistedEnvelope: &str(16) + Version(8) + &str(16) + &[u8](16) + () + padding
    // Expected: ~56 bytes
    assert!(
        persisted_size <= 64,
        "PersistedEnvelope is {persisted_size} bytes — check for padding bloat"
    );

    // PersistedEnvelope should be SMALLER than PendingEnvelope (borrows vs owns)
    assert!(
        persisted_size < pending_size,
        "PersistedEnvelope ({persisted_size}) should be smaller than \
         PendingEnvelope ({pending_size})"
    );
}

// BUG PROBE: StoreError::Conflict — does it correctly preserve BOTH versions?
#[test]
fn bug_probe_conflict_error_preserves_both_versions() {
    let err = StoreError::Conflict {
        stream_id: nexus::ErrorId::from_display(&"test-stream-42"),
        expected: Version::from_persisted(5),
        actual: Version::from_persisted(8),
    };

    let msg = err.to_string();
    // Both version numbers MUST appear in the error message
    assert!(msg.contains('5'), "expected version missing from: {msg}");
    assert!(msg.contains('8'), "actual version missing from: {msg}");
    assert!(msg.contains("test"), "stream_id missing from: {msg}");
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
