//! Kernel benchmarks.
//!
//! Measures the hot paths in the kernel:
//! - apply: single event application throughput
//! - replay: aggregate rehydration with N events
//! - `take_uncommitted_events`: draining events for persistence
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
use nexus::DomainEvent;
use nexus::Id;
use nexus::Message;
use nexus::{Aggregate, AggregateRoot, AggregateState};
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

#[derive(Default, Debug)]
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
    fn name(&self) -> &'static str {
        "Bench"
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
            VersionedEvent::from_persisted(
                Version::from_persisted(u64::try_from(i + 1).expect("index fits in u64")),
                BEvent::Incremented,
            )
        })
        .collect()
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_apply(c: &mut Criterion) {
    c.bench_function("apply (single)", |b| {
        b.iter_with_setup(
            || AggregateRoot::<BAgg>::new(BId(1)),
            |mut agg| {
                agg.apply(black_box(BEvent::Incremented));
                agg
            },
        );
    });
}

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

fn bench_apply_then_take(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_N_then_take");
    for size in [1, 10, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_with_setup(
                || AggregateRoot::<BAgg>::new(BId(1)),
                |mut agg| {
                    for _ in 0..size {
                        agg.apply(BEvent::Incremented);
                    }
                    let events = agg.take_uncommitted_events();
                    black_box(events)
                },
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_apply, bench_replay, bench_apply_then_take);
criterion_main!(benches);
