//! This module defines the `Events` struct and releated types.

#[derive(Debug, PartialEq)]
/// Errors that can occur when creating the `Event` or `EventId`.
pub enum EventError {
    /// Indicates that an invalid ID was provided.
    InvalidId,
    /// Indicates that an empty title was provided.
    EmptyTitle,
}

/// A unique identifier for an event.
#[allow(dead_code)]
#[derive(Debug)]
pub struct EventId(String);

#[allow(dead_code)]
impl EventId {
    /// Creates a new `EventId`.
    ///
    /// # Errors
    ///
    /// Returns an `EventError::InvalidId` if the provided `id` is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::EventId;
    ///
    /// let event_id = EventId::new("event-123".to_string()).unwrap();
    /// assert_eq!(event_id.value(), "event-123");
    /// ```
    pub fn new(id: String) -> Result<Self, EventError> {
        if id.is_empty() {
            return Err(EventError::InvalidId);
        }

        Ok(EventId(id))
    }

    /// Returns the value of the `EventId`.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::EventId;
    /// let event_id = EventId::new("event-123".to_string()).unwrap();
    /// assert_eq!(event_id.value(), "event-123");
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EventId {
    type Error = EventError;

    /// Creates a new `EventId` from a `String`.
    ///
    /// # Errors
    ///
    /// Returns an `EventError::InvalidId` if the provided `id` is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::EventId;
    /// use std::convert::TryFrom;
    ///
    /// let event_id = EventId::try_from("event-123".to_string()).unwrap();
    /// assert_eq!(event_id.value(), "event-123");
    /// ```
    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

/// Represents an event with an ID and a title.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Event {
    id: EventId,
    title: String,
}

#[allow(dead_code)]
impl Event {
    /// Creates a new `Event`.
    ///
    /// # Errors
    ///
    /// Returns an `EventError::EmptyTitle` if the provided `title` is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::{Event, EventId};
    ///
    /// let event_id = EventId::new("event-123".to_string()).unwrap();
    /// let event = Event::new(event_id, "My Event".to_string()).unwrap();
    /// assert_eq!(event.title, "My Event");
    /// ```
    pub fn new(id: EventId, title: String) -> Result<Self, EventError> {
        if title.is_empty() {
            return Err(EventError::EmptyTitle);
        }

        Ok(Event { id, title })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EVENT_ID: &str = "event-id";
    const EVENT_NAME: &str = "event name";

    #[test]
    fn should_create_event_id() {
        let event_id = EventId::new(EVENT_ID.to_string()).unwrap();
        assert_eq!(EVENT_ID, event_id.value());
    }

    #[test]
    fn event_id_should_not_be_created_if_empty() {
        let id = "".to_string();
        let event_id = EventId::new(id);
        assert!(event_id.is_err());
        assert_eq!(event_id.unwrap_err(), EventError::InvalidId);
    }

    #[test]
    fn event_should_be_created() {
        let event_id = EventId::try_from(EVENT_ID.to_string()).unwrap();
        let event = Event::new(event_id, EVENT_NAME.to_string()).unwrap();
        assert_eq!(EVENT_NAME, &event.title);
        assert_eq!(EVENT_ID, event.id.value());
    }

    #[test]
    fn event_not_created_on_empty_title() {
        let event_id = EventId::try_from(EVENT_ID.to_string()).unwrap();
        let event = Event::new(event_id, "".to_string());
        assert!(event.is_err());
        assert_eq!(event.unwrap_err(), EventError::EmptyTitle);
    }
}
