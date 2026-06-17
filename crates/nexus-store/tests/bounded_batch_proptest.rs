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
use nexus_store::batch::BatchSize;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use proptest::prelude::*;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct Tid(String);

impl std::fmt::Display for Tid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<[u8]> for Tid {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl nexus::Id for Tid {
    const BYTE_LEN: usize = 0;
}

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
            let id = Tid("s".into());
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
