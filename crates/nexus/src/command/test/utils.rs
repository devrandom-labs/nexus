use super::UserDomainEvents;
use crate::DomainEvent;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

pub enum EventType {
    Mismatch,
    Ordered,
    UnOrdered,
    Empty,
}

pub fn get_user_events(
    timestamp: Option<DateTime<Utc>>,
    event_type: EventType,
) -> Vec<UserDomainEvents> {
    if let EventType::Empty = event_type {
        return vec![];
    }
    let user_created = timestamp.map_or_else(
        || UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp: Utc::now(),
        },
        |t| UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp: t,
        },
    );

    let user_activated = if let EventType::Mismatch = event_type {
        UserDomainEvents::UserActivated {
            id: "id_1".to_string(),
        }
    } else {
        UserDomainEvents::UserActivated {
            id: "id".to_string(),
        }
    };

    match event_type {
        EventType::Ordered | EventType::Mismatch => vec![user_created, user_activated],
        EventType::UnOrdered => vec![user_activated, user_created],
        _ => vec![],
    }
}

pub fn convert_to_hashmap(events: Vec<UserDomainEvents>) -> HashMap<String, Vec<UserDomainEvents>> {
    let mut map: HashMap<String, Vec<UserDomainEvents>> = HashMap::new();
    for event in events {
        map.entry(event.aggregate_id().to_owned())
            .or_default()
            .push(event)
    }
    map
}

pub struct MockData {
    pub events: Vec<UserDomainEvents>,
    r#type: EventType,
}

impl MockData {
    pub fn new(r#type: EventType, timestamp: Option<DateTime<Utc>>) -> Self {
        let events = if let EventType::Empty = r#type {
            vec![]
        } else {
            let user_created = timestamp.map_or_else(
                || UserDomainEvents::UserCreated {
                    id: "id".to_string(),
                    email: String::from("joel@tixlys.com"),
                    timestamp: Utc::now(),
                },
                |t| UserDomainEvents::UserCreated {
                    id: "id".to_string(),
                    email: String::from("joel@tixlys.com"),
                    timestamp: t,
                },
            );

            let user_activated = if let EventType::Mismatch = r#type {
                UserDomainEvents::UserActivated {
                    id: "id_1".to_string(),
                }
            } else {
                UserDomainEvents::UserActivated {
                    id: "id".to_string(),
                }
            };

            if let EventType::Ordered | EventType::Mismatch = r#type {
                vec![user_created, user_activated]
            } else {
                vec![user_activated, user_created]
            }
        };

        MockData { events, r#type }
    }

    pub fn get_hash_map(self) -> HashMap<String, Vec<UserDomainEvents>> {
        let mut map: HashMap<String, Vec<UserDomainEvents>> = HashMap::new();
        let events = self.events.get(0);
        if let Some(evn) = events {
            if let EventType::Mismatch = self.r#type {
                map.insert(evn.aggregate_id().to_owned(), self.events);
            } else {
                for event in self.events {
                    map.entry(event.aggregate_id().to_owned())
                        .or_default()
                        .push(event)
                }
            }
        }
        map
    }
}
