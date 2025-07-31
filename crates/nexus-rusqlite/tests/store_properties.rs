use nexus::store::EventStore;
use nexus_test_helpers::pending_event_strategies::{
    arbitrary_conflicting_sequence, arbitrary_valid_sequence,
};
use proptest::prelude::*;

mod common;

proptest! {

    #[test]
    fn prop_can_append_a_valid_sequence( events in arbitrary_valid_sequence(1..10)) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = common::TestContext::new();
            let stream_id = *events[0].stream_id();
            let expected_version = events.len() as u64;
            let result = ctx.store.append_to_stream(&stream_id, expected_version, events).await;
            prop_assert!(result.is_ok(), "Appending a valid sequence should succeed");
            Ok(())
        })?;
    }

    #[test]
    fn prop_should_fail_on_shuffled_sequence( events in arbitrary_conflicting_sequence()) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = common::TestContext::new();
            let stream_id = *events[0].stream_id();
            let expected_version = events.len() as u64;
            let result = ctx.store.append_to_stream(&stream_id, expected_version, events).await;
            prop_assert!(result.is_err(), "Appending a shuffled sequence should fail");
            Ok(())
        })?;
    }
}
