use chrono::{DateTime, Utc};
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Tag {
    Music,
    Summer,
    Concert,
}

#[derive(Debug)]
pub enum EventStatus {
    Draft,
    Active,
    Cancelled,
    Postponed,
}

pub struct EventId(Vec<u8>);
pub struct Event {
    id: EventId,
    created_on: DateTime<Utc>,
    title: String,
    description: String,
    start_time: DateTime<Utc>, // optional?
    end_time: DateTime<Utc>,   // optional?
    status: EventStatus,
    tag: Option<HashSet<Tag>>,
}
