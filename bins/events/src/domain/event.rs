use thiserror::Error;
use tracing::{debug, error, info, instrument};
use ulid::Ulid;

/// Represents a unique identifier for an event.
///
/// This struct wraps a [Ulid] to provide a time-sortable, globally unique ID.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct EventId(Ulid);

impl EventId {
    /// Creates a new `EventId` with a randomly generated [Ulid].
    ///
    /// # Examples
    ///
    /// ```
    /// use your_crate::EventId; // Replace your_crate with the actual crate name
    ///
    /// let event_id = EventId::new();
    /// println!("{:?}", event_id); // Output: EventId(...)
    /// ```
    pub fn new() -> Self {
        EventId(Ulid::new())
    }
}

impl Default for EventId {
    /// Creates a new `EventId` using `EventId::new()`.
    ///
    /// This provides a convenient way to generate a default `EventId` when needed.
    fn default() -> Self {
        Self::new()
    }
}

/// Represents an error that can occur when creating an `EventTitle`
#[derive(Debug, Error, PartialEq)]
pub enum TitleError {
    /// Indicates that the title is empty.
    #[error("Title is empty")]
    EmptyTitle,
    /// Indicates that the title is too short (less than 2 characters).
    #[error("Title is too short, should not be less than 2 char")]
    ShortTitle,
    /// Indicates that the title is too long (more than 80 characters).
    #[error("Title is too long, should not be more than 80 char")]
    LongTitle,
}

/// Represents the title of an event.
///
/// This struct ensures that the event title meets certain criteria, such as:
/// - Not being empty
/// - Being at least 2 characters long
/// - Being no more than 80 characters long
/// - Should show meaningful title in unicode format.
#[derive(Debug)]
pub struct EventTitle(String); // should show meaningful title in unicode format.

impl EventTitle {
    /// Creates a new `EventTitle` from a string.
    ///
    /// This function performs the following checks:
    /// 1. Trims whitespace from the input string.
    /// 2. Checks if the trimmed string is empty.
    /// 3. Checks if the trimmed string is less than 2 characters long.
    /// 4. Checks if the trimmed string is more than 80 characters long.
    ///
    /// If any of these checks fail, it returns a corresponding `TitleError`.
    ///
    /// # Arguments
    ///
    /// * `title` - The string to create the `EventTitle` from.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `EventTitle` if successful, or a `TitleError` if any of the checks fail.
    ///
    /// # Examples
    ///
    /// ```
    /// use your_crate::EventTitle; // Replace your_crate with the actual crate name
    ///
    /// let title = EventTitle::new("My Event".to_string()).unwrap();
    /// assert_eq!(title.value(), "My Event");
    ///
    /// let empty_title = EventTitle::new("  ".to_string());
    /// assert!(empty_title.is_err());
    /// ```
    #[instrument]
    pub fn new(title: String) -> Result<Self, TitleError> {
        debug!("creating new event title: {}", title);
        let title = title.trim().to_string();
        if title.is_empty() {
            error!("{}", TitleError::EmptyTitle);
            return Err(TitleError::EmptyTitle);
        }
        if title.len() < 2 {
            error!("{}", TitleError::ShortTitle);
            return Err(TitleError::ShortTitle);
        }
        if title.len() > 80 {
            error!("{}", TitleError::LongTitle);
            return Err(TitleError::LongTitle);
        }
        info!("passes all title checks, creating the title");
        Ok(EventTitle(title))
    }

    /// Returns the underlying string value of the `EventTitle`.
    ///
    /// # Examples
    ///
    /// ```
    /// use your_crate::EventTitle; // Replace your_crate with the actual crate name
    ///
    /// let title = EventTitle::new("My Event".to_string()).unwrap();
    /// assert_eq!(title.value(), "My Event");
    /// ```
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for EventTitle {
    type Error = TitleError;
    /// Attempts to create an `EventTitle` from a string slice.
    ///
    /// This is equivalent to calling `EventTitle::new(title.to_string())`.
    fn try_from(title: &str) -> Result<Self, Self::Error> {
        Self::new(title.into())
    }
}

impl TryFrom<String> for EventTitle {
    type Error = TitleError;
    /// Attempts to create an `EventTitle` from a `String`.
    ///
    /// This is equivalent to calling `EventTitle::new(title)`.
    fn try_from(title: String) -> Result<Self, Self::Error> {
        Self::new(title)
    }
}

impl Default for EventTitle {
    /// Creates a default `EventTitle` with the value "Untitled".
    ///
    /// # Examples
    ///
    /// ```
    /// use your_crate::EventTitle; // Replace your_crate with the actual crate name
    ///
    /// let title = EventTitle::default();
    /// assert_eq!(title.value(), "Untitled");
    /// ```
    #[instrument]
    fn default() -> Self {
        info!("creating default event title");
        Self::try_from("Untitled").unwrap()
    }
}

/// Represents an event.
///
/// An event has a unique `EventId` and an `EventTitle`.
#[derive(Debug, Default)]
pub struct Event {
    /// The unique identifier of the event.
    pub id: EventId,
    /// The title of the event.
    pub title: EventTitle,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT_TITLE: &str = "event_title";
    const LONG_EVENT_TITLE: &str = "this is an extremly long event title and this should always give error, becasue nobody reads a long ass title as this";
    const SHORT_EVENT_TITLE: &str = "1";

    #[test]
    fn default_should_give_ulid() {
        let event_id = EventId::default();
        assert!(!event_id.0.is_nil());
    }

    #[test]
    fn default_title_should_be_untitled() {
        let event_title = EventTitle::default();
        assert_eq!(event_title.value(), "Untitled");
    }
    #[test]
    fn id_can_be_created_by_str() {
        let event_title = EventTitle::try_from(EVENT_TITLE).unwrap();
        assert_eq!(event_title.value(), EVENT_TITLE);
    }
    #[test]
    fn id_can_be_created_by_string() {
        let event_title = EventTitle::try_from(EVENT_TITLE.to_string()).unwrap();
        assert_eq!(event_title.value(), EVENT_TITLE);
    }

    #[test]
    fn id_should_not_be_empty() {
        let event_title = EventTitle::try_from("");
        assert!(event_title.is_err());
        assert_eq!(event_title.err().unwrap(), TitleError::EmptyTitle);
    }
    #[test]
    fn id_should_not_be_more_than_80_char() {
        let event_title = EventTitle::try_from(LONG_EVENT_TITLE);
        assert!(event_title.is_err());
        assert_eq!(event_title.err().unwrap(), TitleError::LongTitle);
    }
    #[test]
    fn id_should_not_be_less_than_1_char() {
        let event_title = EventTitle::try_from(SHORT_EVENT_TITLE);
        assert!(event_title.is_err());
        assert_eq!(event_title.err().unwrap(), TitleError::ShortTitle);
    }
}

// socket servers can have backlog / queue for incoming requests.
