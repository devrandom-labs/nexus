use nexus::store::EventStore;
use nexus_test_helpers::pending_event_strategies::arbitrary_valid_sequence;
use proptest::prelude::*;

mod common;

proptest! {

    #[test]
    fn prop_can_append_a_valid_sequence( events in arbitrary_valid_sequence()) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = common::TestContext::new();
            let stream_id = events[0].stream_id().clone();
            let expected_version = events.len() as u64;
            let result = ctx.store.append_to_stream(&stream_id, expected_version, events).await;
            prop_assert!(result.is_ok(), "Appending a valid sequence should succeed");
            Ok(())
        })?;
    }
}
