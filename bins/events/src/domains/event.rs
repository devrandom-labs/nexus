use chrono::{DateTime, Utc};
use std::collections::HashSet;

/// Represents different categories or themes associated with an event.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Tag {
    /// Music-related event.
    Music,
    /// Summer-themed event.
    Summer,
    /// Concert event.
    Concert,
}

/// Represents the status of an event.
#[derive(Debug)]
pub enum EventStatus {
    /// Event is still being drafted.
    Draft,
    /// Event is active and open for participation.
    Active,
    /// Event has been cancelled.
    Cancelled,
    /// Event has been postponed to a later date.
    Postponed,
}

/// Unique identifier for an event.
#[derive(Debug)]
pub struct EventId(String);

/// Represents an event with details like title, description, time, and status.
#[derive(Debug)]
pub struct Event {
    /// Unique identifier for the event.
    id: EventId,
    /// Timestamp when the event was created.
    created_on: DateTime<Utc>,
    /// Title of the event.
    title: String,
    /// Detailed description of the event.
    description: String,
    /// Start time of the event (optional).
    start_time: DateTime<Utc>,
    /// End time of the event (optional).
    end_time: DateTime<Utc>,
    /// Current status of the event.
    status: EventStatus,
    /// Set of tags associated with the event (optional).
    tags: Option<HashSet<Tag>>,
}
