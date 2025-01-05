#![allow(dead_code)]

#[derive(Default, Debug, PartialEq)]
pub enum EventStatus {
    #[default]
    Draft,
    Cancelled,
    Published,
    Completed,
}

/// Have derived default so initially the state would be empty,
/// to apply events on it and rehydrate it.
/// we can actually deserialize it from the aggregator snapshot, but thats later.
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

#[derive(Debug, PartialEq)]
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

    // rehydrate the aggregator
    // get it from one state to another
    pub fn apply(&mut self, events: Events) {
        match events {
            Events::Created { title, status, id } => {
                self.id = id;
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

    fn get_draft_event() -> EventAggregate {
        let mut ea = EventAggregate::default();
        let history = vec![Events::Created {
            id: "1".to_string(),
            title: "some event".to_string(),
            status: EventStatus::Draft,
        }];
        for event in history {
            ea.apply(event);
        }
        ea
    }

    fn get_published_event() -> EventAggregate {
        let mut ea = EventAggregate::default();
        let history = vec![
            Events::Created {
                id: "1".to_string(),
                title: "some event".to_string(),
                status: EventStatus::Draft,
            },
            Events::Published {
                id: "1".to_string(),
                status: EventStatus::Published,
            },
        ];

        for event in history {
            ea.apply(event);
        }
        ea
    }

    #[test]
    fn create_events_aggregator_with_defaults() {
        let event_aggregator = EventAggregate::default();
        assert_eq!(event_aggregator.id, "".to_string());
        assert_eq!(event_aggregator.title, "".to_string());
        assert_eq!(event_aggregator.status, EventStatus::Draft);
    }

    #[test]
    fn should_create() {
        let ea = EventAggregate::default();
        let create_command = Commands::Create {
            id: "1".to_string(),
            title: "some event".to_string(),
        };

        let output_events = ea.handle(create_command).unwrap();
        let expected_events = vec![Events::Created {
            id: "1".to_string(),
            title: "some event".to_string(),
            status: EventStatus::Draft,
        }];
        assert_eq!(output_events, expected_events);
    }

    #[test]
    fn should_cancel_from_draft() {
        let ea = get_draft_event();
        let cancel_command = Commands::Cancel {
            id: "1".to_string(),
        };
        let output_events = ea.handle(cancel_command).unwrap();
        let expected_events = vec![Events::Cancelled {
            id: "1".to_string(),
            status: EventStatus::Cancelled,
        }];
        assert_eq!(output_events, expected_events);
    }

    #[test]
    fn should_publish() {
        let ea = get_draft_event();
        let publish_command = Commands::Publish {
            id: "1".to_string(),
        };
        let output_events = ea.handle(publish_command).unwrap();
        let expected_events = vec![Events::Published {
            id: "1".to_string(),
            status: EventStatus::Published,
        }];
        assert_eq!(output_events, expected_events);
    }

    #[test]
    fn should_cancel_from_publish() {
        let ea = get_published_event();
        let cancel_command = Commands::Cancel {
            id: "1".to_string(),
        };
        let output_events = ea.handle(cancel_command).unwrap();
        let expected_events = vec![Events::Cancelled {
            id: "1".to_string(),
            status: EventStatus::Cancelled,
        }];
        assert_eq!(output_events, expected_events);
    }

    #[test]
    fn should_complete_form_publish() {
        let ea = get_published_event();
        let command = Commands::Complete {
            id: "1".to_string(),
        };
        let output = ea.handle(command).unwrap();
        let expected = vec![Events::Completed {
            id: "1".to_string(),
            status: EventStatus::Completed,
        }];
        assert_eq!(output, expected);
    }
}

// TODO: compile time type checking that publish events can only have publish status
