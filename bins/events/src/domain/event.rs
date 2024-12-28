#[derive(Debug)]
pub enum EventError {
    InvalidId,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct EventId(pub String);

impl EventId {
    #[allow(dead_code)]
    pub fn new(id: String) -> Result<Self, EventError> {
        if id.is_empty() {
            return Err(EventError::InvalidId);
        }

        Ok(EventId(id))
    }
}
#[allow(dead_code)]
#[derive(Debug)]
pub struct Event {
    id: EventId,
    title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_event_id() {
        let id = "some_id".to_string();

        let event_id = EventId::new(id.clone()).unwrap();
        assert_eq!(id, event_id.0);
    }

    #[test]
    fn event_id_should_not_be_created_if_empty() {
        let id = "".to_string();
        let event_id = EventId::new(id.clone());
        assert!(event_id.is_err());
    }
}
