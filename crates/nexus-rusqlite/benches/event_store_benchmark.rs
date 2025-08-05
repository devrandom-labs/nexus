use criterion::{Bencher, Criterion};
use fake::{Fake, Faker};
use nexus::infra::NexusId;
use nexus::{event::PendingEvent, store::EventStore};
use nexus_rusqlite::Store;
use nexus_test_helpers::pending_event::create_pending_event_sequence;
use refinery::embed_migrations;
use rusqlite::Connection;
use std::hint::black_box;
use std::ops::Range;
use tokio::runtime::Runtime;

embed_migrations!("migrations");

async fn bench_write_with_faker(c: &mut Criterion) {
    let mut conn = Connection::open_in_memory().expect("could not open connection");
    migrations::runner()
        .run(&mut conn)
        .expect("migrations could not be applied.");

    let store = Store::new(conn);
    let rt = Runtime::new().unwrap();
    let stream_id: NexusId = Faker.fake();
    let num_events = 1000;
    let events = rt.block_on(async {
        create_pending_event_sequence(stream_id, 1..num_events)
            .await
            .expect("event sequence generation failed")
    });

    c.bench_function("append_rusqlite_store", |b| {
        b.(|| {
            black_box(store.append_to_stream(black_box(events.clone())))
                .expect("append failed")
                .await
        });
    })
}
