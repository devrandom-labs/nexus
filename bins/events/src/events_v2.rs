#![allow(dead_code)]

pub mod cqrs {
    use std::error::Error;

    pub trait DomainEvents {
        fn get_version(&self) -> String;
    }

    pub trait Aggregate {
        const TYPE: &'static str;
        type Command;
        type Event: DomainEvents;
        type Error: Error;
        fn handle(&self, command: Self::Command) -> Result<Vec<Self::Event>, Self::Error>;
        fn apply(&mut self, event: Self::Event);
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
pub enum Commands {
    Create,
    Cancel,
    Publish,
    Complete,
}

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
