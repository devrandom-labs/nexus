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

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nexus::Version;
use nexus_store::AppendError;
use nexus_store::Upcaster;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BenchId(String);
impl fmt::Display for BenchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl AsRef<[u8]> for BenchId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl nexus::Id for BenchId {
    const BYTE_LEN: usize = 0;
}

fn bid(s: &str) -> BenchId {
    BenchId(s.to_owned())
}

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
    events: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
}

impl EventStream for InMemoryStream {
    type Error = BenchError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.events.len() {
            return Ok(None);
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).unwrap(),
            &row.1,
            1,
            &row.2,
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
        id: &impl nexus::Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(id.to_string()).or_default();
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        let expected_u64 = expected_version.map_or(0, nexus::Version::as_u64);
        if current_version != expected_u64 {
            return Err(AppendError::Store(BenchError::Conflict));
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
        Ok(InMemoryStream { events, pos: 0 })
    }
}

// =============================================================================
// Noop upcaster for benchmarks — walks v1 through v6
// =============================================================================

struct NoopV1ToV6;

impl Upcaster for NoopV1ToV6 {
    type Error = Infallible;

    fn apply<'a>(
        &self,
        mut morsel: nexus_store::upcasting::EventMorsel<'a>,
    ) -> Result<nexus_store::upcasting::EventMorsel<'a>, nexus_store::UpcastError<Self::Error>>
    {
        loop {
            morsel = match (morsel.event_type(), morsel.schema_version()) {
                ("UserCreated", v) if v == Version::new(1).unwrap() => {
                    nexus_store::upcasting::EventMorsel::new(
                        "UserCreated",
                        Version::new(2).unwrap(),
                        morsel.payload().to_vec(),
                    )
                }
                ("UserCreated", v) if v == Version::new(2).unwrap() => {
                    nexus_store::upcasting::EventMorsel::new(
                        "UserCreated",
                        Version::new(3).unwrap(),
                        morsel.payload().to_vec(),
                    )
                }
                ("UserCreated", v) if v == Version::new(3).unwrap() => {
                    nexus_store::upcasting::EventMorsel::new(
                        "UserCreated",
                        Version::new(4).unwrap(),
                        morsel.payload().to_vec(),
                    )
                }
                ("UserCreated", v) if v == Version::new(4).unwrap() => {
                    nexus_store::upcasting::EventMorsel::new(
                        "UserCreated",
                        Version::new(5).unwrap(),
                        morsel.payload().to_vec(),
                    )
                }
                ("UserCreated", v) if v == Version::new(5).unwrap() => {
                    nexus_store::upcasting::EventMorsel::new(
                        "UserCreated",
                        Version::new(6).unwrap(),
                        morsel.payload().to_vec(),
                    )
                }
                _ => break,
            };
        }
        Ok(morsel)
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "UserCreated" => Some(Version::new(6).unwrap()),
            _ => None,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn make_envelopes(n: usize) -> Vec<PendingEnvelope<()>> {
    (0..n)
        .map(|i| {
            let version = u64::try_from(i + 1).unwrap_or(u64::MAX);
            pending_envelope(Version::new(version).unwrap())
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
                pending_envelope(black_box(Version::INITIAL))
                    .event_type("UserCreated")
                    .payload(vec![1, 2, 3, 4])
                    .build_without_metadata(),
            )
        });
    });
}

fn bench_persisted_envelope_construction(c: &mut Criterion) {
    let event_type = "UserCreated";
    let payload = [1u8, 2, 3, 4, 5, 6, 7, 8];
    c.bench_function("PersistedEnvelope::new_unchecked (zero-alloc)", |b| {
        b.iter(|| {
            black_box(PersistedEnvelope::<()>::new_unchecked(
                Version::INITIAL,
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
                        .append(&bid("bench-stream"), None, black_box(envs))
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
                .append(&bid("bench-stream"), None, &envelopes)
                .await
                .unwrap();
        });

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let mut stream = store
                        .read_stream(&bid("bench-stream"), Version::INITIAL)
                        .await
                        .unwrap();
                    while let Some(env) = stream.next().await.unwrap() {
                        black_box(env);
                    }
                });
            });
        });
    }
    group.finish();
}

fn bench_upcaster(c: &mut Criterion) {
    let upcaster = NoopV1ToV6;
    let payload = vec![1u8, 2, 3, 4, 5, 6, 7, 8];

    c.bench_function("upcaster (5 noop steps)", |b| {
        b.iter(|| {
            let morsel = nexus_store::EventMorsel::borrowed(
                "UserCreated",
                Version::new(1).unwrap(),
                black_box(&payload),
            );
            let result = upcaster.apply(morsel).unwrap();
            black_box(result)
        });
    });
}

criterion_group!(
    benches,
    bench_builder_throughput,
    bench_persisted_envelope_construction,
    bench_append,
    bench_read_stream,
    bench_upcaster
);
criterion_main!(benches);
