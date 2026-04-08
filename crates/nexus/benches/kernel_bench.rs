//! Kernel benchmarks.
//!
//! Measures the hot paths in the kernel:
//! - replay: aggregate rehydration with N events
//! - `apply_events`: post-persistence state advancement
//!
//! Run: `cargo bench --bench kernel_bench`
//! Reports: `target/criterion/report/index.html`
#![allow(clippy::unwrap_used, reason = "benchmarks use unwrap for brevity")]
#![allow(clippy::expect_used, reason = "benchmarks use expect for brevity")]
#![allow(
    clippy::shadow_reuse,
    reason = "closure parameter shadowing is idiomatic in criterion benchmarks"
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nexus::{Aggregate, AggregateRoot, AggregateState, DomainEvent, Events, Id, Message};
use nexus::{Version, VersionedEvent};
use std::fmt;
use std::hint::black_box;

// =============================================================================
// Bench domain — minimal types, fast apply
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BId(u64);
impl fmt::Display for BId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for BId {}

#[derive(Debug, Clone)]
#[allow(dead_code, reason = "Set variant exists for realistic bench domain")]
enum BEvent {
    Incremented,
    Set(u64),
}
impl Message for BEvent {}
impl DomainEvent for BEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Incremented => "Incremented",
            Self::Set(_) => "Set",
        }
    }
}

#[derive(Default, Debug, Clone)]
struct BState {
    count: u64,
}
impl AggregateState for BState {
    type Event = BEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &BEvent) -> Self {
        match event {
            BEvent::Incremented => self.count += 1,
            BEvent::Set(v) => self.count = *v,
        }
        self
    }
}

#[derive(Debug)]
struct BAgg;
#[derive(Debug, thiserror::Error)]
#[error("bench error")]
struct BErr;
impl Aggregate for BAgg {
    type State = BState;
    type Error = BErr;
    type Id = BId;
}

// =============================================================================
// Helpers
// =============================================================================

fn make_versioned_events(n: usize) -> Vec<VersionedEvent<BEvent>> {
    (0..n)
        .map(|i| {
            VersionedEvent::new(
                Version::new(u64::try_from(i + 1).expect("index fits in u64")).unwrap(),
                BEvent::Incremented,
            )
        })
        .collect()
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_replay(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay");
    for size in [10, 100, 1_000, 10_000] {
        let events = make_versioned_events(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &events, |b, events| {
            b.iter(|| {
                let mut agg = AggregateRoot::<BAgg>::new(black_box(BId(1)));
                for ve in events {
                    agg.replay(ve.version(), ve.event()).unwrap();
                }
                agg
            });
        });
    }
    group.finish();
}

fn bench_apply_events(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_events");
    for size in [1, 10, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_with_setup(
                || {
                    let agg = AggregateRoot::<BAgg>::new(BId(1));
                    let batches: Vec<Events<BEvent>> = (0..size)
                        .map(|_| Events::new(BEvent::Incremented))
                        .collect();
                    (agg, batches)
                },
                |(mut agg, batches)| {
                    for (i, batch) in batches.iter().enumerate() {
                        #[allow(clippy::as_conversions, reason = "bench index always fits u64")]
                        let v = Version::new((i + 1) as u64).unwrap();
                        agg.advance_version(v);
                        agg.apply_events(batch);
                    }
                    black_box(agg)
                },
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_replay, bench_apply_events);
criterion_main!(benches);
