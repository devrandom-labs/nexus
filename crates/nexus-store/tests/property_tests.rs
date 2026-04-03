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

use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use std::collections::HashMap;
use tokio::sync::Mutex;

use proptest::prelude::*;

// ============================================================================
// In-memory adapter (same pattern as raw_store_tests.rs)
// ============================================================================

/// Row stored per event: (version, `event_type`, payload).
type StoredRow = (u64, String, Vec<u8>);

/// Minimal in-memory adapter for testing `RawEventStore`.
struct InMemoryRawStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemoryRawStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

struct InMemoryStream {
    events: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl EventStream for InMemoryStream {
    type Error = TestError;

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

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("concurrency conflict")]
    Conflict,
}

impl RawEventStore for InMemoryRawStore {
    type Error = TestError;
    type Stream<'a>
        = InMemoryStream
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
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_version != expected_version.as_u64() {
            return Err(TestError::Conflict);
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        drop(guard);
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
        Ok(InMemoryStream { events, pos: 0 })
    }
}

// ============================================================================
// Helper: build envelopes from payloads
// ============================================================================

/// Leaked static str for `event_type` (proptest generates many values; we leak
/// to satisfy the `&'static str` requirement on `PendingEnvelope`).
fn leak_event_type(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn build_envelopes(stream_id: &str, payloads: &[Vec<u8>]) -> Vec<PendingEnvelope<()>> {
    payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let version_num = u64::try_from(i).unwrap_or(u64::MAX) + 1;
            pending_envelope(stream_id.to_owned())
                .version(Version::from_persisted(version_num))
                .event_type(leak_event_type("TestEvent"))
                .payload(p.clone())
                .build_without_metadata()
        })
        .collect()
}

// ============================================================================
// Property-based tests
// ============================================================================

fn stream_id_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z][a-z0-9-]{0,19}").unwrap()
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
            let store = InMemoryRawStore::new();
            let envelopes = build_envelopes(&stream_id, &payloads);

            store
                .append(&stream_id, Version::INITIAL, &envelopes)
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
            let store = InMemoryRawStore::new();
            let payloads: Vec<Vec<u8>> = (0..n).map(|i| vec![u8::try_from(i % 256).unwrap_or(0)]).collect();
            let envelopes = build_envelopes(&stream_id, &payloads);

            store
                .append(&stream_id, Version::INITIAL, &envelopes)
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
            let store = InMemoryRawStore::new();

            let envelopes_a = build_envelopes(&id_a, &payloads_a);
            let envelopes_b = build_envelopes(&id_b, &payloads_b);

            store
                .append(&id_a, Version::INITIAL, &envelopes_a)
                .await
                .unwrap();
            store
                .append(&id_b, Version::INITIAL, &envelopes_b)
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

    /// Chaining two upcasters (V1->V2 and V2->V3) produces final version 3
    /// with payload preserved.
    #[test]
    fn upcaster_chain_composition(
        payload in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        struct V1ToV2;
        impl EventUpcaster for V1ToV2 {
            fn can_upcast(&self, _: &str, v: u32) -> bool {
                v == 1
            }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), 2, p.to_vec())
            }
        }

        struct V2ToV3;
        impl EventUpcaster for V2ToV3 {
            fn can_upcast(&self, _: &str, v: u32) -> bool {
                v == 2
            }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), 3, p.to_vec())
            }
        }

        let chain: Vec<Box<dyn EventUpcaster>> =
            vec![Box::new(V1ToV2), Box::new(V2ToV3)];

        let event_type = "E";
        let mut current_version: u32 = 1;
        let mut current_payload = payload.clone();
        let mut current_event_type = event_type.to_owned();

        for upcaster in &chain {
            if upcaster.can_upcast(&current_event_type, current_version) {
                let (new_type, new_version, new_payload) =
                    upcaster.upcast(&current_event_type, current_version, &current_payload);
                current_event_type = new_type;
                current_version = new_version;
                current_payload = new_payload;
            }
        }

        prop_assert_eq!(current_version, 3, "final version should be 3");
        prop_assert_eq!(
            current_event_type, "E",
            "event type should be preserved"
        );
        prop_assert_eq!(
            current_payload, payload,
            "payload should be preserved through upcaster chain"
        );
    }
}
