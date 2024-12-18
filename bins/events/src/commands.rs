use chrono::{DateTime, Utc};

pub struct CreateEvent {
    created_on: DateTime<Utc>,
    payload: Event,
}

struct Event {
    title: String,
    description: String,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
}
