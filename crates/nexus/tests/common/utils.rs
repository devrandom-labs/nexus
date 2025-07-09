use super::write_side_setup::UserDomainEvents;
use chrono::{DateTime, Utc};
use nexus::{domain::DomainEvent, infra::NexusId};
use std::collections::HashMap;

pub enum EventType {
    Mismatch,
    Ordered,
    UnOrdered,
    Empty,
}

pub struct MockData {
    pub events: Vec<UserDomainEvents>,
    r#type: EventType,
}

impl MockData {
    pub fn new(timestamp: Option<DateTime<Utc>>, r#type: EventType) -> Self {
        let events = if let EventType::Empty = r#type {
            vec![]
        } else {
            let user_created = timestamp.map_or_else(
                || UserDomainEvents::UserCreated {
                    id: NexusId::default(),
                    email: String::from("joel@tixlys.com"),
                    timestamp: NexusId::default(),
                },
                |t| UserDomainEvents::UserCreated {
                    id: NexusId::default(),
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
        let events = self.events.first();
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
