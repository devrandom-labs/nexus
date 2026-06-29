//! Property: a bounded paginating read yields exactly the stream's events,
//! in order, regardless of how `batch_size` chunks them.
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "test code"
)]

use futures::StreamExt;
use nexus::Version;
use nexus_store::StreamKey;
use nexus_store::batch::BatchSize;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use proptest::prelude::*;

fn batch_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        Just(1usize),
        Just(2),
        Just(255),
        Just(256),
        Just(257),
        1usize..512
    ]
}

fn len_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(0u64),
        Just(1),
        Just(255),
        Just(256),
        Just(257),
        0u64..600
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn read_yields_all_in_order(batch in batch_strategy(), len in len_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryStore::with_batch_size(BatchSize::new(batch).unwrap());
            let id = StreamKey::from_slice(b"s");
            for v in 1..=len {
                let env = pending_envelope(Version::new(v).unwrap())
                    .event_type("E")
                    .payload(vec![(v % 256) as u8])
                    .unwrap()
                    .build();
                store.append(&id, Version::new(v - 1), &[env]).await.unwrap();
            }
            let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
            let mut seen = Vec::new();
            while let Some(item) = stream.next().await {
                seen.push(item.unwrap().version().as_u64());
            }
            prop_assert_eq!(seen, (1..=len).collect::<Vec<_>>());
            Ok(())
        })?;
    }
}
