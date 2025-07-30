use nexus::{event::PendingEvent, infra::NexusId};
use proptest::prelude::*;

pub type TestPendingEvent = PendingEvent<NexusId>;

// TODO: valid_sequence
// TODO: invalid_sequence
// TODO: multiple_stream_valid_sequence
// TODO: multiple_stream_invalid_sequence

fn valid_sequence() -> impl Strategy<Value = Vec<TestPendingEvent>> {
    // stream_id strategy
    let stream_id_strategy = any::<[u8; 16]>().prop_map(|bytes| {
        let uuid = uuid::Builder::from_random_bytes(bytes).into_uuid();
        Into::<NexusId>::into(uuid)
    });

    (stream_id_strategy, 0..10_usize)
        .prop_flat_map(|(stream_id, num_events)| {
            (
                Just(stream_id),
                prop::collection::vec((any::<String>(), any::<Vec<u8>>()), num_events),
            )
        })
        .prop_map(|(stream_id, other_data)| {
            other_data
                .into_iter()
                .enumerate()
                .map(|(index, (event_type, payload))| todo!())
                .collect::<Vec<_>>()
        })
}
