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

#[derive(Debug)]
pub struct EventId(String);

#[derive(Debug)]
pub struct Event {
    id: EventId,
    created_on: DateTime<Utc>,
    title: String,
    description: String,
    start_time: DateTime<Utc>, // optional?
    end_time: DateTime<Utc>,   // optional?
    status: EventStatus,
    tags: Option<HashSet<Tag>>,
}
