use super::UserDomainEvents;
use crate::DomainEvent;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

pub enum EventType {
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
    let user_activated = UserDomainEvents::UserActivated {
        id: "id".to_string(),
    };
    match event_type {
        EventType::Ordered => vec![user_created, user_activated],
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
