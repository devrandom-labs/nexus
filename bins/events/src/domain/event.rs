use thiserror::Error;
use ulid::Ulid;

#[derive(Debug, Error)]
pub enum EventError {
    #[error("Event title is empty")]
    EmptyTitle,
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

#[derive(Debug)]
pub struct EventTitle(String); // should show meaningful title in unicode format.

impl EventTitle {
    pub fn value(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_should_give_ulid() {
        let event_id = EventId::default();
        assert!(!event_id.0.is_nil());
    }

    #[test]
    fn default_title_should_be_untitled() {}
    #[test]
    fn id_can_be_created_by_str() {}
    #[test]
    fn id_can_be_created_by_string() {}
    #[test]
    fn id_should_not_be_empty() {}
    #[test]
    fn id_should_not_be_more_than_80_char() {}
    #[test]
    fn id_should_not_be_less_than_1_char() {}
}
