//! `nexus-store` benchmarks.
//!
//! Measures the hot paths in the store layer:
//! - `PendingEnvelope` builder throughput
//! - `PersistedEnvelope` construction (zero-alloc)
//! - append throughput at various batch sizes
//! - `read_stream` throughput at various sizes
//! - upcaster chain throughput
//!
//! Run: `cargo bench --bench store_bench -p nexus-store`
//! Reports: `target/criterion/report/index.html`
#![allow(clippy::unwrap_used, reason = "benchmarks use unwrap for brevity")]
#![allow(
    clippy::as_conversions,
    reason = "benchmarks use as casts for index conversions"
)]
#![allow(
    clippy::str_to_string,
    reason = "benchmarks use to_string for convenience"
)]
#![allow(clippy::print_stdout, reason = "criterion may print to stdout")]
#![allow(clippy::print_stderr, reason = "criterion may print to stderr")]
#![allow(
    clippy::missing_panics_doc,
    reason = "benchmark functions do not need panic docs"
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use std::collections::HashMap;
use std::hint::black_box;
use tokio::sync::Mutex;

// =============================================================================
// In-memory adapter (same pattern as raw_store_tests.rs)
// =============================================================================

type StoredRow = (u64, String, Vec<u8>);

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
    type Error = BenchError;

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
enum BenchError {
    #[error("concurrency conflict")]
    Conflict,
}

impl RawEventStore for InMemoryRawStore {
    type Error = BenchError;
    type Stream<'a> = InMemoryStream;

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
            return Err(BenchError::Conflict);
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

// =============================================================================
// Noop upcaster for chain benchmarks
// =============================================================================

struct NoopUpcaster {
    target_type: &'static str,
    target_version: u32,
}

impl EventUpcaster for NoopUpcaster {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        event_type == self.target_type && schema_version == self.target_version
    }

    fn upcast(
        &self,
        _event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        (
            self.target_type.to_owned(),
            schema_version + 1,
            payload.to_vec(),
        )
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn make_envelopes(n: usize) -> Vec<PendingEnvelope<()>> {
    (0..n)
        .map(|i| {
            let version = u64::try_from(i + 1).unwrap_or(u64::MAX);
            pending_envelope("bench-stream".to_owned())
                .version(Version::from_persisted(version))
                .event_type("BenchEvent")
                .payload(vec![1, 2, 3, 4])
                .build_without_metadata()
        })
        .collect()
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_builder_throughput(c: &mut Criterion) {
    c.bench_function("PendingEnvelope builder", |b| {
        b.iter(|| {
            black_box(
                pending_envelope(black_box("stream-1".to_owned()))
                    .version(Version::from_persisted(1))
                    .event_type("UserCreated")
                    .payload(vec![1, 2, 3, 4])
                    .build_without_metadata(),
            )
        });
    });
}

fn bench_persisted_envelope_construction(c: &mut Criterion) {
    let stream_id = "stream-1";
    let event_type = "UserCreated";
    let payload = [1u8, 2, 3, 4, 5, 6, 7, 8];
    c.bench_function("PersistedEnvelope::new (zero-alloc)", |b| {
        b.iter(|| {
            black_box(PersistedEnvelope::<()>::new(
                black_box(stream_id),
                Version::from_persisted(1),
                black_box(event_type),
                1,
                black_box(&payload),
                (),
            ))
        });
    });
}

fn bench_append(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("append");

    for size in [1, 10, 100, 1000] {
        let envelopes = make_envelopes(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &envelopes, |b, envs| {
            b.iter(|| {
                let store = InMemoryRawStore::new();
                rt.block_on(async {
                    store
                        .append("bench-stream", Version::INITIAL, black_box(envs))
                        .await
                        .unwrap();
                });
            });
        });
    }
    group.finish();
}

fn bench_read_stream(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_stream");

    for size in [10, 100, 1000] {
        let envelopes = make_envelopes(size);

        // Pre-populate the store once for this size
        let store = InMemoryRawStore::new();
        rt.block_on(async {
            store
                .append("bench-stream", Version::INITIAL, &envelopes)
                .await
                .unwrap();
        });

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let mut stream = store
                        .read_stream("bench-stream", Version::INITIAL)
                        .await
                        .unwrap();
                    while let Some(result) = stream.next().await {
                        black_box(result.unwrap());
                    }
                });
            });
        });
    }
    group.finish();
}

fn bench_upcaster_chain(c: &mut Criterion) {
    // Chain of 5 noop upcasters: v1->v2, v2->v3, ..., v5->v6
    let upcasters: Vec<Box<dyn EventUpcaster>> = (1..=5)
        .map(|v| -> Box<dyn EventUpcaster> {
            Box::new(NoopUpcaster {
                target_type: "UserCreated",
                target_version: v,
            })
        })
        .collect();

    let payload = vec![1u8, 2, 3, 4, 5, 6, 7, 8];

    c.bench_function("upcaster chain (5 noop)", |b| {
        b.iter(|| {
            let mut event_type = "UserCreated".to_owned();
            let mut schema_version: u32 = 1;
            let mut current_payload = payload.clone();

            for upcaster in &upcasters {
                if upcaster.can_upcast(&event_type, schema_version) {
                    let (new_type, new_version, new_payload) =
                        upcaster.upcast(&event_type, schema_version, &current_payload);
                    event_type = new_type;
                    schema_version = new_version;
                    current_payload = new_payload;
                }
            }

            black_box((event_type, schema_version, current_payload))
        });
    });
}

criterion_group!(
    benches,
    bench_builder_throughput,
    bench_persisted_envelope_construction,
    bench_append,
    bench_read_stream,
    bench_upcaster_chain
);
criterion_main!(benches);
