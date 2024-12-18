use chrono::{DateTime, Utc};

pub struct CreateEvent {
    title: String,
    created_on: DateTime<Utc>,
}
