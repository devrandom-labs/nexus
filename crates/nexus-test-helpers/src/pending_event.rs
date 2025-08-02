use crate::UserEvents;
use fake::{Fake, Faker};
use nexus::{
    error::Result,
    event::{BoxedEvent, EventMetadata, PendingEvent},
    infra::{CorrelationId, NexusId},
};
use proptest::{collection::vec as prop_vec, prelude::*, sample::SizeRange};
use std::ops::RangeBounds;

pub type TestPendingEvent = PendingEvent<NexusId>;

// TODO: multiple_stream_valid_sequence
// TODO: multiple_stream_invalid_sequence
const EVENT_TYPE_STRATEGY: &str = "[a-z0-9]{1,20}";

pub fn arbitrary_correlation_id() -> impl Strategy<Value = CorrelationId> {
    any::<[u8; 16]>().prop_map(|bytes| {
        let uuid = uuid::Builder::from_random_bytes(bytes)
            .into_uuid()
            .to_string();
        Into::<CorrelationId>::into(uuid)
    })
}

pub fn arbitrary_event_metadata() -> impl Strategy<Value = EventMetadata> {
    arbitrary_correlation_id().prop_map(EventMetadata::new)
}

pub fn arbitrary_stream_id() -> impl Strategy<Value = NexusId> {
    any::<[u8; 16]>().prop_map(|bytes| {
        let uuid = uuid::Builder::from_random_bytes(bytes).into_uuid();
        Into::<NexusId>::into(uuid)
    })
}

pub fn arbitrary_valid_sequence<R>(size: R) -> impl Strategy<Value = Vec<TestPendingEvent>>
where
    R: RangeBounds<usize> + Strategy,
    SizeRange: From<R::Value>,
{
    (arbitrary_stream_id(), size)
        .prop_flat_map(move |(stream_id, num_events)| {
            (
                Just(stream_id),
                prop_vec(
                    (
                        arbitrary_event_metadata(),
                        EVENT_TYPE_STRATEGY,
                        any::<Vec<u8>>(),
                    ),
                    num_events,
                ),
            )
        })
        .prop_map(|(stream_id, other_data)| {
            other_data
                .into_iter()
                .enumerate()
                .map(|(index, (metadata, event_type, payload))| {
                    PendingEvent::builder(stream_id)
                        .with_version((index + 1) as u64)
                        .unwrap()
                        .with_metadata(metadata)
                        .build_with_payload(payload, event_type)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
}

pub fn arbitrary_conflicting_sequence() -> impl Strategy<Value = Vec<TestPendingEvent>> {
    arbitrary_valid_sequence(2..10)
        .prop_flat_map(|base_sequence| {
            let indices_strategy = prop_vec(0..base_sequence.len(), 1..=base_sequence.len());
            (Just(base_sequence), indices_strategy)
        })
        .prop_map(|(mut base_sequence, indices)| {
            for index in indices {
                base_sequence.push(base_sequence[index].clone());
            }

            base_sequence
        })
        .prop_shuffle()
}

pub async fn create_pending_event(
    stream_id: &NexusId,
    version: u64,
) -> Result<PendingEvent<NexusId>> {
    let event: BoxedEvent = Faker.fake::<UserEvents>().into();
    let metadata: EventMetadata = Faker.fake();
    PendingEvent::builder(stream_id.clone())
        .with_version(version)?
        .with_metadata(metadata)
        .with_domain(event)
        .build(|_domain_event| async move { Ok(fake::vec![u8; 10]) })
        .await
}

pub async fn create_pending_event_sequence(
    stream_id: &NexusId,
    range: impl RangeBounds<usize>,
) -> Result<Vec<PendingEvent<NexusId>>> {
    let stream_id: NexusId = Faker.fake();
    for version in &range {
        todo!()
    }

    todo!()
}
