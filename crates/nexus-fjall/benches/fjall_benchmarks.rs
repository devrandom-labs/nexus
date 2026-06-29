#![allow(clippy::unwrap_used, reason = "bench code")]
#![allow(clippy::expect_used, reason = "bench code")]
#![allow(clippy::missing_panics_doc, reason = "bench code")]
#![allow(
    clippy::shadow_reuse,
    reason = "criterion closure parameter shadowing is idiomatic"
)]

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::StreamExt;
use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_store::StreamKey;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::value::{EventType, Payload, SchemaVersion};
use nexus_store::wire;
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sk(s: &str) -> StreamKey {
    StreamKey::from_slice(s.as_bytes())
}

fn make_envelope(version: u64, payload: &[u8]) -> nexus_store::PendingEnvelope {
    pending_envelope(Version::new(version).unwrap())
        .event_type("BenchEvent")
        .payload(payload.to_vec())
        .expect("valid payload")
        .build()
}

fn make_envelopes(count: u64, payload: &[u8]) -> Vec<nexus_store::PendingEnvelope> {
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
// 1. Wire-frame encoding benchmarks (pure computation, no I/O)
//
// Measures the REAL production value encoder/decoder
// (`nexus_store::wire::encode_frame` / `decode_frame`) that `FjallStore::append`
// and the read cursor drive — not an adapter-local test wrapper (CLAUDE.md
// rule 8). The fjall key codecs (`encode_event_key`, `encode_stream_version`)
// are crate-private; their cost is exercised end-to-end by the `append_*` and
// `read_stream` benchmarks below.
// ---------------------------------------------------------------------------

fn build_frame_value(event_type: &str, p: &[u8]) -> Bytes {
    let sv = SchemaVersion::from_u32(1).unwrap();
    let et = EventType::from_bytes(Bytes::copy_from_slice(event_type.as_bytes())).unwrap();
    let pl = Payload::from_bytes(Bytes::copy_from_slice(p)).unwrap();
    wire::encode_frame(1, sv, &et, &pl, None).unwrap().value
}

fn encoding_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoding");

    let sv = SchemaVersion::from_u32(1).unwrap();
    let et = EventType::from_bytes(Bytes::from_static(b"BenchEvent")).unwrap();

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let pl = Payload::from_bytes(Bytes::from(payload(size))).unwrap();
        group.bench_with_input(BenchmarkId::new("encode_frame", label), &pl, |b, pl| {
            b.iter(|| wire::encode_frame(1, sv, &et, pl, None).unwrap());
        });
    }

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let encoded = build_frame_value("BenchEvent", &payload(size));
        group.bench_with_input(
            BenchmarkId::new("decode_frame", label),
            &encoded,
            |b, encoded| {
                b.iter(|| wire::decode_frame(encoded.as_ref()).unwrap());
            },
        );
    }

    for &(label, size) in &[
        ("small_64B", SMALL_PAYLOAD),
        ("medium_1KB", MEDIUM_PAYLOAD),
        ("large_64KB", LARGE_PAYLOAD),
    ] {
        let pl = Payload::from_bytes(Bytes::from(payload(size))).unwrap();
        group.bench_with_input(
            BenchmarkId::new("encode_decode_roundtrip", label),
            &pl,
            |b, pl| {
                b.iter(|| {
                    let frame = wire::encode_frame(1, sv, &et, pl, None).unwrap();
                    wire::decode_frame(frame.value.as_ref()).unwrap();
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
                            .append(&sk("bench-stream"), None, &envs)
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
                            .append(&sk("bench-stream"), expected, &[env])
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
                    .append(&sk("bench-stream"), None, &envs)
                    .await
                    .unwrap();
            });

            b.iter(|| {
                rt.block_on(async {
                    let mut stream = store
                        .read_stream(&sk("bench-stream"), Version::INITIAL)
                        .await
                        .unwrap();
                    while let Some(item) = stream.next().await {
                        let _ = item.unwrap();
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
                                    let id =
                                        StreamKey::from_slice(format!("stream-{i}").as_bytes());
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
