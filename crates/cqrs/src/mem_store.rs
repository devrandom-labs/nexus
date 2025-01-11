use crate::store::Store;
use std::collections::HashMap;
use thiserror::Error;

type EventHashMap = HashMap<String, Vec<String>>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("Events not found for this `{0}` aggregator")]
    NoEventsFound(String),
}

#[derive(Clone)]
pub struct MemStore {
    events: EventHashMap,
}

impl MemStore {
    pub fn init(initial: Option<EventHashMap>) -> Self {
        let events = initial.unwrap_or_default();
        MemStore { events }
    }
}

impl Store for MemStore {
    type Error = StoreError;
    fn get_events(&self, id: &str) -> Result<Vec<String>, Self::Error> {
        self.events
            .get(id)
            .ok_or(StoreError::NoEventsFound(id.into()))
            .cloned()
    }

    fn commit(&self, aggregate_type: &str, events: &[String]) -> Result<(), Self::Error> {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGGREGATE_TYPE: &str = "event_1";

    fn get_test_data() -> MemStore {
        let mut initial_data: EventHashMap = HashMap::new();
        initial_data.insert(
            String::from(AGGREGATE_TYPE),
            vec![String::from("1"), String::from("2")],
        );
        MemStore::init(Some(initial_data))
    }

    #[test]
    fn get_events_for_an_aggregate_type() {
        let mem_store = get_test_data();
        let events = mem_store.get_events(AGGREGATE_TYPE).unwrap();
        let expected: Vec<String> = vec!["1".to_string(), "2".to_string()];
        assert_eq!(events, expected);
    }

    #[test]
    fn fail_when_there_are_no_events_for_aggregate() {
        let mem_store = MemStore::init(None);
        let result = mem_store.get_events(AGGREGATE_TYPE);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            StoreError::NoEventsFound(AGGREGATE_TYPE.to_string())
        );
    }
}
