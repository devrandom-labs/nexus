use criterion::async_executor::FuturesExecutor;
use criterion::{Bencher, Criterion};
use fake::{Fake, Faker};
use nexus::infra::NexusId;
use nexus_test_helpers::pending_event::create_pending_event_sequence;
use std::ops::Range;
use tokio::runtime::Runtime;

fn bench_write_with_faker(c: &mut Criterion) {
    c.bench_function("event_store_write_100_faked_events", |_b: &mut Bencher| {
        let rt = Runtime::new().unwrap();

        let _events_to_write = rt.block_on(async {
            let stream_id: NexusId = Faker.fake();
            let _events = create_pending_event_sequence(stream_id, Range { start: 1, end: 100 })
                .await
                .unwrap();
        });
    })
}
