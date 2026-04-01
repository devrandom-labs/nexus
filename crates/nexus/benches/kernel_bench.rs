//! Kernel benchmarks.
//!
//! Measures the hot paths in the kernel:
//! - apply_event: single event application throughput
//! - load_from_events: aggregate rehydration with N events
//! - take_uncommitted_events: draining events for persistence
//!
//! Run: cargo bench --bench kernel_bench
//! Reports: target/criterion/report/index.html

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use nexus::kernel::aggregate::{Aggregate, AggregateRoot, AggregateState, EventOf};
use nexus::kernel::event::DomainEvent;
use nexus::kernel::id::Id;
use nexus::kernel::message::Message;
use nexus::kernel::version::{Version, VersionedEvent};
use std::fmt;

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
enum BEvent {
    Incremented,
    Set(u64),
}
impl Message for BEvent {}
impl DomainEvent for BEvent {
    fn name(&self) -> &'static str {
        match self {
            BEvent::Incremented => "Incremented",
            BEvent::Set(_) => "Set",
        }
    }
}

#[derive(Default, Debug)]
struct BState {
    count: u64,
}
impl AggregateState for BState {
    type Event = BEvent;
    fn apply(&mut self, event: &BEvent) {
        match event {
            BEvent::Incremented => self.count += 1,
            BEvent::Set(v) => self.count = *v,
        }
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
        .map(|i| VersionedEvent::from_persisted(Version::from_persisted((i + 1) as u64), BEvent::Incremented))
        .collect()
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_apply_event(c: &mut Criterion) {
    c.bench_function("apply_event (single)", |b| {
        b.iter_with_setup(
            || AggregateRoot::<BAgg>::new(BId(1)),
            |mut agg| {
                agg.apply_event(black_box(BEvent::Incremented));
                agg
            },
        );
    });
}

fn bench_load_from_events(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_from_events");
    for size in [10, 100, 1_000, 10_000] {
        let events = make_versioned_events(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &events, |b, events| {
            b.iter_with_setup(
                || events.iter().map(|ve| {
                    VersionedEvent::from_persisted(ve.version(), ve.event().clone())
                }).collect::<Vec<_>>(),
                |events| {
                    AggregateRoot::<BAgg>::load_from_events(black_box(BId(1)), events).unwrap()
                },
            );
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
                        agg.apply_event(BEvent::Incremented);
                    }
                    let events = agg.take_uncommitted_events();
                    black_box(events)
                },
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_apply_event, bench_load_from_events, bench_apply_then_take);
criterion_main!(benches);
