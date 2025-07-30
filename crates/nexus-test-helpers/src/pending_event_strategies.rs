use nexus::{
    event::{EventMetadata, PendingEvent},
    infra::{CorrelationId, NexusId},
};
use proptest::prelude::*;

pub type TestPendingEvent = PendingEvent<NexusId>;

// TODO: invalid_sequence
// TODO: multiple_stream_valid_sequence
// TODO: multiple_stream_invalid_sequence

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

pub fn arbitrary_valid_sequence() -> impl Strategy<Value = Vec<TestPendingEvent>> {
    // stream_id strategy
    (arbitrary_stream_id(), 1..10_usize)
        .prop_flat_map(|(stream_id, num_events)| {
            (
                Just(stream_id),
                prop::collection::vec(
                    (
                        arbitrary_event_metadata(),
                        any::<String>(),
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
                        .with_metadata(metadata)
                        .build_with_payload(payload, event_type)
                })
                .collect::<Vec<_>>()
        })
}
