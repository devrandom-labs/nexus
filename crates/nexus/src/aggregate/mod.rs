use crate::{DomainEvent, Message};
use serde::{Serialize, de::DeserializeOwned};
use std::{fmt::Debug, hash::Hash};

pub mod aggregate_root;
pub mod command_handler;

pub use aggregate_root::AggregateRoot;
pub use command_handler::AggregateCommandHandler;

/// Represents the actual state data of an aggregate.
/// It's rebuilt by applying events and must know how to apply them.
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    /// Specific type of Domain Event this state reacts to.
    type Event: Message + Clone + Serialize + DeserializeOwned;

    /// Mutates the state based on a receieved event.
    /// This is the core of state reconstruction in Event Sourcing.
    /// It should *NOT* contain complex logic or validation. just state mutation.
    fn apply(&mut self, event: &Self::Event);
}

/// Marker trait to define the component types of a specific Aggregate kind.
/// Implement this on a distinct type (often a unit struct) for each aggregate.
pub trait AggregateType: Send + Sync + Debug + Copy + Clone + 'static {
    /// The type used to uniquely identify this aggregate instance.
    type Id: Send + Sync + Debug + Hash + Eq + 'static + Clone;
    /// The specific type of Domain Event associated with this aggregate.
    type Event: DomainEvent<Id = Self::Id> + PartialEq;
    /// The specific type representing the internal state data.
    /// Crucially links the State's Event type to the AggregateType's Event type.
    type State: AggregateState<Event = Self::Event>;
}

/// Provides a standard interface for accessing aggregate properties.
/// Implemented by `AggregateRoot`.
pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Send + Sync + Debug + Hash + Eq + 'static + Clone;
    type Event: DomainEvent<Id = Self::Id> + PartialEq;
    type State: AggregateState<Event = Self::Event>;

    fn id(&self) -> &Self::Id;
    /// Returns the version loaded from the store (basis for concurrency checks).
    fn version(&self) -> u64;
    fn state(&self) -> &Self::State;
    /// Takes ownership of newly generated events for saving.
    fn take_uncommitted_events(&mut self) -> Vec<Self::Event>;
}

pub trait Service: Debug + Sync + Send {}

#[cfg(test)]
pub mod test {
    use super::AggregateState;
    use crate::{DomainEvent, Message};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, PartialEq)]
    pub struct UserState {
        email: Option<String>,
        is_active: bool,
        created_at: Option<DateTime<Utc>>,
    }

    impl UserState {
        pub fn new(
            email: Option<String>,
            is_active: bool,
            created_at: Option<DateTime<Utc>>,
        ) -> Self {
            Self {
                email,
                is_active,
                created_at,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum UserDomainEvents {
        UserCreated {
            id: String,
            email: String,
            timestamp: DateTime<Utc>,
        },
        UserActivated {
            id: String,
        },
    }

    impl Message for UserDomainEvents {}

    impl DomainEvent for UserDomainEvents {
        type Id = String;
        fn aggregate_id(&self) -> &Self::Id {
            match self {
                Self::UserActivated { id } => id,
                Self::UserCreated { id, .. } => id,
            }
        }
    }

    impl AggregateState for UserState {
        type Event = UserDomainEvents;

        fn apply(&mut self, event: &Self::Event) {
            match event {
                UserDomainEvents::UserCreated {
                    email, timestamp, ..
                } => {
                    self.email = Some(email.to_string());
                    self.created_at = Some(*timestamp);
                }
                UserDomainEvents::UserActivated { .. } => {
                    if self.created_at.is_some() {
                        self.is_active = true;
                    }
                }
            }
        }
    }

    pub enum EventOrderType {
        Ordered,
        UnOrdered,
    }

    pub fn get_user_events(
        timestamp: Option<DateTime<Utc>>,
        order: EventOrderType,
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
        match order {
            EventOrderType::Ordered => vec![user_created, user_activated],
            EventOrderType::UnOrdered => vec![user_activated, user_created],
        }
    }

    #[test]
    fn user_state_default() {
        let user_state = UserState::default();
        assert_eq!(user_state.email, None);
        assert_eq!(user_state.created_at, None);
        assert!(!user_state.is_active);
    }

    #[test]
    fn user_state_apply_created() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        let user_created = UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp,
        };

        user_state.apply(&user_created);

        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
    }

    #[test]
    fn user_state_apply_activated() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        for events in get_user_events(Some(timestamp), EventOrderType::Ordered) {
            user_state.apply(&events);
        }
        assert!(user_state.is_active);
    }

    #[test]
    fn user_state_apply_order() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        for events in get_user_events(Some(timestamp), EventOrderType::UnOrdered) {
            user_state.apply(&events);
        }
        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
        assert!(!user_state.is_active);
    }

    #[test]
    fn user_state_apply_idempotency() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        let user_created = UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp,
        };
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
        assert!(!user_state.is_active);
    }
}
