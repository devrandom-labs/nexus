use chrono::{DateTime, Utc};

pub struct EventId(Vec<u8>);
pub struct Event {
    id: EventId,
    created_on: DateTime<Utc>,
    title: String,
    description: String,
    start_time: DateTime<Utc>, // optional?
    end_time: DateTime<Utc>,   // optional?
}
