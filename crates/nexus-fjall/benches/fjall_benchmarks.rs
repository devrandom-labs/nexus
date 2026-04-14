#![allow(clippy::unwrap_used, reason = "bench code")]
#![allow(clippy::expect_used, reason = "bench code")]
#![allow(clippy::missing_panics_doc, reason = "bench code")]
#![allow(
    clippy::shadow_reuse,
    reason = "criterion closure parameter shadowing is idiomatic"
)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_fjall::encoding::{
    decode_event_key, decode_event_value, decode_stream_meta, encode_event_key, encode_event_value,
    encode_stream_meta,
};
use nexus_store::PendingEnvelope;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use std::fmt;
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
impl nexus::Id for BenchId {}
fn bid(s: &str) -> BenchId {
    BenchId(s.to_owned())
}

fn make_envelope(version: u64, payload: &[u8]) -> PendingEnvelope<()> {
    pending_envelope(Version::new(version).unwrap())
        .event_type("BenchEvent")
        .payload(payload.to_vec())
        .build_without_metadata()
}

fn make_envelopes(count: u64, payload: &[u8]) -> Vec<PendingEnvelope<()>> {
    (1..=count).map(|v| make_envelope(v, payload)).collect()
}

fn temp_store() -> (FjallStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

const SMALL_PAYLOAD: usize = 64;
const MEDIUM_PAYLOAD: usize = 1024;
const LARGE_PAYLOAD: usize = 65_536;

fn payload(size: usize) -> Vec<u8> {
    vec![0xAB; size]
}

// ---------------------------------------------------------------------------
// 1. Encoding benchmarks (pure computation, no I/O)
// ---------------------------------------------------------------------------

fn encoding_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoding");

    group.bench_function("encode_event_key", |b| {
        b.iter(|| encode_event_key(42, 100));
    });

    let key = encode_event_key(42, 100);
    group.bench_function("decode_event_key", |b| {
        b.iter(|| decode_event_key(&key).unwrap());
    });

    group.bench_function("encode_stream_meta", |b| {
        b.iter(|| encode_stream_meta(7, 999));
    });

    let meta = encode_stream_meta(7, 999);
    group.bench_function("decode_stream_meta", |b| {
        b.iter(|| decode_stream_meta(&meta).unwrap());
    });

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let p = payload(size);
        group.bench_with_input(BenchmarkId::new("encode_event_value", label), &p, |b, p| {
            let mut buf = Vec::with_capacity(size + 64);
            b.iter(|| {
                encode_event_value(&mut buf, 1, "BenchEvent", p).unwrap();
            });
        });
    }

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let p = payload(size);
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 1, "BenchEvent", &p).unwrap();
        let encoded = buf.clone();
        group.bench_with_input(
            BenchmarkId::new("decode_event_value", label),
            &encoded,
            |b, encoded| {
                b.iter(|| decode_event_value(encoded).unwrap());
            },
        );
    }

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let p = payload(size);
        group.bench_with_input(
            BenchmarkId::new("encode_decode_roundtrip", label),
            &p,
            |b, p| {
                let mut buf = Vec::with_capacity(size + 64);
                b.iter(|| {
                    encode_event_value(&mut buf, 1, "BenchEvent", p).unwrap();
                    decode_event_value(&buf).unwrap();
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Append throughput — single batch (isolates encoding + write from fsync)
// ---------------------------------------------------------------------------

fn append_batch_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("append_batch");
    let event_payload = payload(SMALL_PAYLOAD);

    for &count in &[10u64, 100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(count));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_with_setup(
                || {
                    let (store, dir) = temp_store();
                    let envs = make_envelopes(count, &event_payload);
                    (store, dir, envs)
                },
                |(store, _dir, envs)| {
                    rt.block_on(async {
                        store
                            .append(&bid("bench-stream"), None, &envs)
                            .await
                            .unwrap();
                    });
                },
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Append throughput — many transactions (realistic per-command cost)
// ---------------------------------------------------------------------------

fn append_sequential_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("append_sequential");
    let event_payload = payload(SMALL_PAYLOAD);

    for &count in &[10u64, 50, 100] {
        group.throughput(Throughput::Elements(count));
        group.sample_size(10);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_with_setup(temp_store, |(store, _dir)| {
                rt.block_on(async {
                    for v in 1..=count {
                        let env = make_envelope(v, &event_payload);
                        let expected = if v == 1 { None } else { Version::new(v - 1) };
                        store
                            .append(&bid("bench-stream"), expected, &[env])
                            .await
                            .unwrap();
                    }
                });
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Read throughput — pre-populated store, measure pure read speed
// ---------------------------------------------------------------------------

fn read_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_stream");
    let event_payload = payload(SMALL_PAYLOAD);

    for &count in &[10u64, 100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(count));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            let (store, _dir) = temp_store();
            let envs = make_envelopes(count, &event_payload);
            rt.block_on(async {
                store
                    .append(&bid("bench-stream"), None, &envs)
                    .await
                    .unwrap();
            });

            b.iter(|| {
                rt.block_on(async {
                    let mut stream = store
                        .read_stream(&bid("bench-stream"), Version::INITIAL)
                        .await
                        .unwrap();
                    while let Some(result) = stream.next().await {
                        let _ = result.unwrap();
                    }
                });
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. Lifecycle benchmarks — open/reopen cost
// ---------------------------------------------------------------------------

fn lifecycle_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("lifecycle");
    group.sample_size(10);

    let event_payload = payload(SMALL_PAYLOAD);

    group.bench_function("open_empty_database", |b| {
        b.iter_with_setup(
            || tempfile::tempdir().unwrap(),
            |dir| {
                let _store = FjallStore::builder(dir.path().join("db")).open().unwrap();
            },
        );
    });

    for &stream_count in &[10u64, 100, 1_000] {
        group.bench_with_input(
            BenchmarkId::new("reopen_with_streams", stream_count),
            &stream_count,
            |b, &stream_count| {
                b.iter_with_setup(
                    || {
                        let dir = tempfile::tempdir().unwrap();
                        let db_path = dir.path().join("db");
                        {
                            let store = FjallStore::builder(&db_path).open().unwrap();
                            rt.block_on(async {
                                for i in 0..stream_count {
                                    let id = BenchId(format!("stream-{i}"));
                                    let env = make_envelope(1, &event_payload);
                                    store.append(&id, None, &[env]).await.unwrap();
                                }
                            });
                            drop(store);
                        }
                        (dir, db_path)
                    },
                    |(_dir, db_path)| {
                        let _store = FjallStore::builder(&db_path).open().unwrap();
                    },
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    encoding_benchmarks,
    append_batch_throughput,
    append_sequential_throughput,
    read_throughput,
    lifecycle_benchmarks,
);
criterion_main!(benches);
