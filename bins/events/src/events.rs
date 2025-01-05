#[allow(dead_code)]
#[derive(Default, Debug, PartialEq)]
pub enum EventStatus {
    #[default]
    Draft,
    Cancelled,
    Published,
    Completed,
    Deleted,
}

#[allow(dead_code)]
#[derive(Default)]
pub struct EventAggregate {
    id: String,
    title: String,
    status: EventStatus,
}

#[allow(dead_code)]
pub enum Commands {
    CreateEvent { id: String, title: String },
    DeleteEvent { id: String },
}

#[allow(dead_code)]
pub enum Events {
    EventCreated {
        id: String,
        title: String,
        status: EventStatus,
    },
    EventDeleted {
        id: String,
        status: EventStatus,
    },
}

#[allow(dead_code)]
impl EventAggregate {
    pub fn new(id: String, title: String, status: EventStatus) -> Self {
        EventAggregate { id, title, status }
    }

    pub fn handle(&self, commands: Commands) -> Result<Vec<Events>, String> {
        match commands {
            Commands::CreateEvent { id, title } => Ok(vec![Events::EventCreated {
                id,
                title,
                status: EventStatus::Draft,
            }]),
            Commands::DeleteEvent { id } => Ok(vec![Events::EventDeleted {
                id,
                status: EventStatus::Deleted,
            }]),
        }
    }
    pub fn apply(&mut self, events: Events) {
        match events {
            Events::EventCreated { title, status, .. } => {
                self.title = title;
                self.status = status;
            }
            Events::EventDeleted { status, .. } => {
                self.status = status;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT_ID: &str = "event-id";
    const EVENT_TITLE: &str = "event title";

    #[test]
    fn create_events_aggregator_with_defaults() {
        let status = EventStatus::default();
        assert_eq!(status, EventStatus::Draft);
        let event_aggregator =
            EventAggregate::new(EVENT_ID.to_string(), EVENT_TITLE.to_string(), status);

        assert_eq!(event_aggregator.id, EVENT_ID);
        assert_eq!(event_aggregator.title, EVENT_TITLE);
    }
}
