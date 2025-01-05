#![allow(dead_code)]

#[derive(Default, Debug, PartialEq)]
pub enum EventStatus {
    #[default]
    Draft,
    Cancelled,
    Published,
    Completed,
}

#[derive(Default, Debug)]
pub struct EventAggregate {
    id: String,
    title: String,
    status: EventStatus,
}

#[derive(Debug)]
pub enum Commands {
    Create { id: String, title: String },
    Cancel { id: String },
    Publish { id: String },
    Complete { id: String },
}

#[derive(Debug)]
pub enum Events {
    Created {
        id: String,
        title: String,
        status: EventStatus,
    },

    Cancelled {
        id: String,
        status: EventStatus,
    },

    Completed {
        id: String,
        status: EventStatus,
    },

    Published {
        id: String,
        status: EventStatus,
    },
}

impl EventAggregate {
    pub fn new(id: String, title: String, status: EventStatus) -> Self {
        EventAggregate { id, title, status }
    }

    pub fn handle(&self, commands: Commands) -> Result<Vec<Events>, String> {
        match commands {
            Commands::Create { id, title } => Ok(vec![Events::Created {
                id,
                title,
                status: EventStatus::Draft,
            }]),

            Commands::Cancel { id } => {
                if let EventStatus::Published | EventStatus::Draft = &self.status {
                    Ok(vec![Events::Cancelled {
                        id,
                        status: EventStatus::Cancelled,
                    }])
                } else {
                    Err("Cannot cancel event".to_string())
                }
            }
            Commands::Complete { id } => {
                if let EventStatus::Published = &self.status {
                    Ok(vec![Events::Completed {
                        id,
                        status: EventStatus::Completed,
                    }])
                } else {
                    Err("Cannot complete event".to_string())
                }
            }
            Commands::Publish { id } => {
                if let EventStatus::Draft = &self.status {
                    Ok(vec![Events::Published {
                        id,
                        status: EventStatus::Published,
                    }])
                } else {
                    Err("Cannot publish event".to_string())
                }
            }
        }
    }
    pub fn apply(&mut self, events: Events) {
        match events {
            Events::Created { title, status, .. } => {
                self.title = title;
                self.status = status;
            }
            Events::Completed { status, .. }
            | Events::Published { status, .. }
            | Events::Cancelled { status, .. } => self.status = status,
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

    #[test]
    fn should_emit_created_event() {
        unimplemented!()
    }

    #[test]
    fn should_emit_cancelled_event() {
        unimplemented!()
    }

    #[test]
    fn should_emit_published_event() {
        unimplemented!()
    }

    #[test]
    fn should_emit_completed_event() {}
}
