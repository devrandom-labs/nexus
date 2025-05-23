use super::UserDomainEvents;
use chrono::{DateTime, Utc};

pub enum EventType {
    Ordered,
    UnOrdered,
    Empty,
}

pub fn get_user_events(
    timestamp: Option<DateTime<Utc>>,
    event_type: EventType,
) -> Vec<UserDomainEvents> {
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
        EventType::Empty => vec![],
    }
}
