use thiserror::Error;
use ulid::Ulid;

#[derive(Debug, Error)]
pub enum EventError {
    #[error("{0}")]
    Title(String),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct EventId(Ulid);

impl EventId {
    pub fn new() -> Self {
        EventId(Ulid::new())
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum TitleError {
    #[error("Title is empty")]
    EmptyTitle,
    #[error("Title is too short, should not be less than 2 char")]
    ShortTitle,
    #[error("Title is too long, should not be more than 80 char")]
    LongTitle,
}

#[derive(Debug)]
pub struct EventTitle(String); // should show meaningful title in unicode format.

impl EventTitle {
    pub fn new(title: String) -> Result<Self, TitleError> {
        let title = title.trim().to_string();
        // series of checks on title.
        if title.is_empty() {
            return Err(TitleError::EmptyTitle);
        }
        if title.len() < 2 {
            return Err(TitleError::ShortTitle);
        }
        if title.len() > 80 {
            return Err(TitleError::LongTitle);
        }
        Ok(EventTitle(title))
    }
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for EventTitle {
    type Error = TitleError;
    fn try_from(title: &str) -> Result<Self, Self::Error> {
        Self::new(title.into())
    }
}

impl TryFrom<String> for EventTitle {
    type Error = TitleError;

    fn try_from(title: String) -> Result<Self, Self::Error> {
        Self::new(title)
    }
}

impl Default for EventTitle {
    fn default() -> Self {
        Self::try_from("Untitled").unwrap()
    }
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
