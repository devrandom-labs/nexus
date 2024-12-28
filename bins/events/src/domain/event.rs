#[derive(Debug)]
pub enum EventError {
    InvalidId,
    EmptyTitle,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct EventId(String);

#[allow(dead_code)]
impl EventId {
    pub fn new(id: String) -> Result<Self, EventError> {
        if id.is_empty() {
            return Err(EventError::InvalidId);
        }

        Ok(EventId(id))
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EventId {
    type Error = EventError;
    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct Event {
    id: EventId,
    title: String,
}

#[allow(dead_code)]
impl Event {
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

    #[test]
    fn should_create_event_id() {
        let id = "some_id";

        let event_id = EventId::new(id.to_string()).unwrap();
        assert_eq!(id, event_id.value());
    }

    #[test]
    fn event_id_should_not_be_created_if_empty() {
        let id = "".to_string();
        let event_id = EventId::new(id.clone());
        assert!(event_id.is_err());
    }
}
