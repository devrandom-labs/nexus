#![allow(dead_code)]

pub mod cqrs {
    use std::collections::HashMap;
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

    pub trait Store {
        type Error;
        // provided an aggregator type  id, get all events for that id.
        fn get_events(&self, id: String) -> Result<Vec<String>, Self::Error>;
    }

    pub struct MemStore {
        events: HashMap<String, String>,
    }

    impl Store for MemStore {
        type Error: Error;
        fn get_events(&self, id: String) -> Result<Vec<String>, Error> {
            self.events.get(&id).ok_or(Err("cant find"))?
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
    },
    Completed {
        id: EventId,
        completed_at: DateTime<Utc>,
    },
}
