use ulid::Ulid;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_should_give_ulid() {
        let event_id = EventId::default();
        assert!(!event_id.0.is_nil());
    }
}
