use nexus_test_helpers::pending_event_strategies::arbitrary_valid_sequence;
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_can_append_a_valid_sequence( _events in arbitrary_valid_sequence()) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            unimplemented!()
        })
    }
}
