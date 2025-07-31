use nexus::{
    event::{EventMetadata, PendingEvent},
    infra::{CorrelationId, NexusId},
};
use proptest::{
    collection::{hash_set, vec as prop_vec},
    prelude::*,
    sample::SizeRange,
    strategy::ValueTree,
};
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

// cretae multiple stream_ids and for each srteam create multiple events

pub fn arbitrary_multi_stream_valid_sequence<R>() -> impl Strategy<Value = Vec<TestPendingEvent>>
where
    R: RangeBounds<usize> + Strategy,
    SizeRange: From<R::Value>,
{
    (2..=5_usize)
        .prop_flat_map(|num_streams| hash_set(arbitrary_stream_id(), num_streams))
        .prop_flat_map(|stream_ids| {
            stream_ids
                .into_iter()
                .map(|id| (Just(id), 1..10_usize))
                .collect::<Vec<_>>()
        })
        .prop_map(|stream_configs| {
            let all_events: Vec<TestPendingEvent> = stream_configs
                .into_iter()
                .flat_map(|(stream_id, num_events)| {
                    (1..=num_events).map(move |version| {
                        let metadata = arbitrary_event_metadata()
                            .new_tree(&mut Default::default())
                            .unwrap()
                            .current();

                        let event_type = EVENT_TYPE_STRATEGY
                            .new_tree(&mut Default::default())
                            .unwrap()
                            .current();

                        let payload = any::<Vec<u8>>()
                            .new_tree(&mut Default::default())
                            .unwrap()
                            .current();

                        PendingEvent::builder(stream_id)
                            .with_version(version as u64)
                            .unwrap()
                            .with_metadata(metadata)
                            .build_with_payload(payload, event_type)
                            .unwrap()
                    })
                })
                .collect();

            all_events
        })
        .prop_shuffle()
}
