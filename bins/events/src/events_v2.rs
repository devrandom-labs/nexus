#![allow(dead_code)]

pub mod cqrs {

    use std::error::Error;

    pub trait DomainEvents {
        fn get_version(&self) -> String;
    }
    pub trait Aggregate {
        const TYPE: &'static str;
        type Event: DomainEvents;
        type Error: Error;
        fn apply(&mut self, event: Self::Event);
    }

    pub mod store {
        use std::collections::HashMap;
        use std::error::Error as StdError;
        use thiserror::Error;

        #[derive(Debug, Error, PartialEq, Eq)]
        pub enum StoreError {
            #[error("Events not found for this `{0}` aggregator")]
            NoEventsFound(String),
        }

        pub trait Store {
            type Error: StdError;
            fn get_events(&self, id: &str) -> Result<Vec<String>, Self::Error>;
        }

        type EventHashMap = HashMap<String, Vec<String>>;

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
    }
}

pub mod domain {
    use chrono::{DateTime, Utc};
    use ulid::Ulid;

    pub struct Draft;
    pub struct Cancelled {
        cancelled_at: DateTime<Utc>,
    }
    pub struct Completed {
        completed_at: DateTime<Utc>,
    }
    pub struct Published {
        published_at: DateTime<Utc>,
    }

    pub trait EventState {}
    impl EventState for Draft {}
    impl EventState for Cancelled {}
    impl EventState for Completed {}
    impl EventState for Published {}

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
    pub struct EventId(Ulid);

    impl EventId {
        pub fn new() -> Self {
            EventId(Ulid::new())
        }
    }

    pub struct EventAggregate<S: EventState> {
        id: Option<EventId>,
        status: S,
    }
}

use chrono::{DateTime, Utc};
type EventId = domain::EventId;

#[derive(Debug)]
pub enum Events {
    Created {
        id: EventId,
    },
    Cancelled {
        id: EventId,
        cancelled_at: DateTime<Utc>,
    },
    Published {
        id: EventId,
        published_at: DateTime<Utc>,