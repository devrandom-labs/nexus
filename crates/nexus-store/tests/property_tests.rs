#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for clarity and brevity"
)]
#![allow(
    clippy::expect_used,
    reason = "tests use expect for clarity and brevity"
)]
#![allow(clippy::str_to_string, reason = "tests use to_string/to_owned freely")]
#![allow(
    clippy::shadow_reuse,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::shadow_unrelated,
    reason = "tests shadow variables for readability"
)]
#![allow(clippy::panic, reason = "proptest macros use panic internally")]
#![allow(
    clippy::missing_panics_doc,
    reason = "proptest macro generates code that triggers this lint"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "proptest macro generates code that triggers this lint"
)]

use std::fmt;

use nexus::Version;
use nexus_store::Upcaster;
use nexus_store::envelope::PendingEnvelope;
use nexus_store::pending_envelope;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;

use proptest::prelude::*;

// ============================================================================
// Test ID type
// ============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl nexus::Id for TestId {}

// ============================================================================
// Helper: build envelopes from payloads
// ============================================================================

/// Leaked static str for `event_type` (proptest generates many values; we leak
/// to satisfy the `&'static str` requirement on `PendingEnvelope`).
fn leak_event_type(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn build_envelopes(payloads: &[Vec<u8>]) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let version_num = u64::try_from(i).unwrap_or(u64::MAX) + 1;
            pending_envelope(Version::new(version_num).unwrap())
                .event_type(leak_event_type("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

// ============================================================================
// Property-based tests
// ============================================================================

fn stream_id_strategy() -> impl Strategy<Value = TestId> {
    prop::string::string_regex("[a-z][a-z0-9-]{0,19}")
        .unwrap()
        .prop_map(TestId)
}

fn payloads_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
    prop::collection::vec(prop::collection::vec(any::<u8>(), 0..256), 1..20)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any valid sequence of payloads, appending them and reading back
    /// yields identical payloads in the same order.
    #[test]
    fn append_read_roundtrip(
        stream_id in stream_id_strategy(),
        payloads in payloads_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let envelopes = build_envelopes(&payloads);

            store
                .append(&stream_id, None, &envelopes)
                .await
                .unwrap();

            let mut stream = store
                .read_stream(&stream_id, Version::INITIAL)
                .await
                .unwrap();

            let mut read_payloads: Vec<Vec<u8>> = Vec::new();
            while let Some(result) = stream.next().await {
                let env = result.unwrap();
                read_payloads.push(env.payload().to_vec());
            }

            prop_assert_eq!(read_payloads.len(), payloads.len());
            for (read, original) in read_payloads.iter().zip(payloads.iter()) {
                prop_assert_eq!(read, original);
            }
            Ok(())
        })?;
    }

    /// For any N events appended, `read_stream` always returns events in
    /// ascending version order.
    #[test]
    fn read_returns_ascending_versions(
        stream_id in stream_id_strategy(),
        n in 1..50usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![u8::try_from(i % 256).unwrap_or(0)]).collect();
            let envelopes = build_envelopes(&payloads);

            store
                .append(&stream_id, None, &envelopes)
                .await
                .unwrap();

            let mut stream = store
                .read_stream(&stream_id, Version::INITIAL)
                .await
                .unwrap();

            let mut prev_version: u64 = 0;
            while let Some(result) = stream.next().await {
                let env = result.unwrap();
                let current = env.version().as_u64();
                prop_assert!(
                    current > prev_version,
                    "versions must be strictly ascending: got {} after {}",
                    current,
                    prev_version,
                );
                prev_version = current;
            }
            Ok(())
        })?;
    }

    /// Events appended to stream "a" never appear when reading stream "b"
    /// and vice versa.
    #[test]
    fn stream_isolation(
        id_a in stream_id_strategy(),
        id_b in stream_id_strategy(),
        payloads_a in payloads_strategy(),
        payloads_b in payloads_strategy(),
    ) {
        // Ensure distinct stream IDs
        prop_assume!(id_a != id_b);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::new();

            let envelopes_a = build_envelopes(&payloads_a);
            let envelopes_b = build_envelopes(&payloads_b);

            store
                .append(&id_a, None, &envelopes_a)
                .await
                .unwrap();
            store
                .append(&id_b, None, &envelopes_b)
                .await
                .unwrap();

            // Read stream A — should contain only A's payloads
            {
                let mut stream = store.read_stream(&id_a, Version::INITIAL).await.unwrap();
                let mut count = 0usize;
                while let Some(result) = stream.next().await {
                    let env = result.unwrap();
                    let idx = count;
                    prop_assert!(
                        idx < payloads_a.len(),
                        "stream A returned more events than appended"
                    );
                    prop_assert_eq!(
                        env.payload().to_vec(),
                        payloads_a[idx].clone(),
                        "stream A returned wrong payload at index {}",
                        idx,
                    );
                    count += 1;
                }
                prop_assert_eq!(
                    count,
                    payloads_a.len(),
                    "stream A returned wrong number of events"
                );
            }

            // Read stream B — should contain only B's payloads
            {
                let mut stream = store.read_stream(&id_b, Version::INITIAL).await.unwrap();
                let mut count = 0usize;
                while let Some(result) = stream.next().await {
                    let env = result.unwrap();
                    let idx = count;
                    prop_assert!(
                        idx < payloads_b.len(),
                        "stream B returned more events than appended"
                    );
                    prop_assert_eq!(
                        env.payload().to_vec(),
                        payloads_b[idx].clone(),
                        "stream B returned wrong payload at index {}",
                        idx,
                    );
                    count += 1;
                }
                prop_assert_eq!(
                    count,
                    payloads_b.len(),
                    "stream B returned wrong number of events"
                );
            }

            Ok(())
        })?;
    }

    /// Upcaster that walks V1->V2->V3 produces final version 3 with payload preserved.
    #[test]
    fn upcaster_composition(
        payload in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        struct V1ToV3Upcaster;
        impl Upcaster for V1ToV3Upcaster {
            fn apply<'a>(&self, mut morsel: nexus_store::upcasting::EventMorsel<'a>) -> Result<nexus_store::upcasting::EventMorsel<'a>, nexus_store::UpcastError> {
                loop {
                    morsel = match (morsel.event_type(), morsel.schema_version()) {
                        ("E", v) if v == Version::INITIAL => nexus_store::upcasting::EventMorsel::new("E", Version::new(2).unwrap(), morsel.payload().to_vec()),
                        ("E", v) if v == Version::new(2).unwrap() => nexus_store::upcasting::EventMorsel::new("E", Version::new(3).unwrap(), morsel.payload().to_vec()),
                        _ => break,
                    };
                }
                Ok(morsel)
            }
            fn current_version(&self, event_type: &str) -> Option<Version> {
                match event_type {
                    "E" => Some(Version::new(3).unwrap()),
                    _ => None,
                }
            }
        }

        let morsel = nexus_store::EventMorsel::borrowed("E", Version::INITIAL, &payload);
        let result = V1ToV3Upcaster.apply(morsel).unwrap();

        prop_assert_eq!(result.schema_version(), Version::new(3).unwrap(), "final version should be 3");
        prop_assert_eq!(
            result.event_type(), "E",
            "event type should be preserved"
        );
        prop_assert_eq!(
            result.payload(), payload.as_slice(),
            "payload should be preserved through upcaster"
        );
    }
}
