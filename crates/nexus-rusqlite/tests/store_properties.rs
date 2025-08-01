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
            let result = ctx.store.append_to_stream(0, events).await;
            prop_assert!(result.is_ok(), "Appending a valid sequence should succeed");
            Ok(())
        })?;
    }

    #[test]
    fn prop_should_fail_on_conflicting_sequence( events in arbitrary_conflicting_sequence()) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = common::TestContext::new();
            let result = ctx.store.append_to_stream(0, events).await;
            prop_assert!(result.is_err(), "Appending a shuffled sequence should fail");
            Ok(())
        })?;
    }

    // #[test]
    // fn prop_can_append_multiple_stream_id_with_valid_sequence(events in arbitrary_multi_stream_valid_sequence()) {
    //     tokio::runtime::Runtime::new().unwrap().block_on(async {
    //         let ctx = common::TestContext::new();
    //         // if i have multi stream event batch how can I append??
    //         Ok(())
    //     })?;
    // }

    // #[test]
    // fn prop_should_fail_on_multi_stream_conflicting_sequence() {}
}

//
// TODO: The Generalization (Property Test): Elevate the simple case to a universal law.
// TODO: The Chaos (Fuzz Test): Attack the boundaries with invalid and malicious data.
// TODO: The Structure (Snapshot Test): Ensure the physical data format remains stable.
// TODO: The Audit (Mutation Test): Test the quality of our other tests.
