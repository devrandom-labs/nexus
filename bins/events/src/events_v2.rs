#![allow(dead_code)]

pub mod status {
    use chrono::{DateTime, Utc};

    pub struct Draft {
        created_at: DateTime<Utc>,
    }
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
}

pub struct EventId(String);

pub struct EventAggregate<S: status::EventState> {
    id: Option<EventId>,
    status: S,
}

impl EventAggregate<status::Draft> {}
impl EventAggregate<status::Cancelled> {}
