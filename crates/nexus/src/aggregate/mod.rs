use crate::DomainEvent;
use std::{fmt::Debug, hash::Hash};

pub mod aggregate_root;
pub mod command_handler;

pub use aggregate_root::AggregateRoot;
pub use command_handler::AggregateCommandHandler;

/// Represents the actual state data of an aggregate.
/// It's rebuilt by applying events and must know how to apply them.
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    /// Specific type of Domain Event this state reacts to.
    type Event: DomainEvent;

    /// Mutates the state based on a receieved event.
    /// This is the core of state reconstruction in Event Sourcing.
    /// It should *NOT* contain complex logic or validation. just state mutation.
    fn apply(&mut self, event: &Self::Event);
}

/// Marker trait to define the component types of a specific Aggregate kind.
/// Implement this on a distinct type (often a unit struct) for each aggregate.
pub trait AggregateType: Send + Sync + Debug + Copy + Clone + 'static {
    /// The type used to uniquely identify this aggregate instance.
    type Id: Send + Sync + Debug + Eq + Hash + Clone + 'static;
    /// The specific type of Domain Event associated with this aggregate.
    type Event: DomainEvent + PartialEq;
    /// The specific type representing the internal state data.
    /// Crucially links the State's Event type to the AggregateType's Event type.
    type State: AggregateState<Event = Self::Event>;
}

/// Provides a standard interface for accessing aggregate properties.
/// Implemented by `AggregateRoot`.
pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Send + Sync + Debug + Eq + Hash + Clone + 'static;
    type Event: DomainEvent + PartialEq;
    type State: AggregateState<Event = Self::Event>;

    fn id(&self) -> &Self::Id;
    /// Returns the version loaded from the store (basis for concurrency checks).
    fn version(&self) -> u64;
    fn state(&self) -> &Self::State;
    /// Takes ownership of newly generated events for saving.
    fn take_uncommitted_events(&mut self) -> Vec<Self::Event>;
}

// // Marker type for User Aggregate
// #[derive(Debug, Clone, Copy)]
// pub struct UserAggregateType;
// impl AggregateType for UserAggregateType {
//     type Id = String;
//     type State = UserState;
//     type Event = UserEvent;
// }

// // User State
// #[derive(Debug, Clone, Default)]
// pub struct UserState {
//     email: Option<String>,
//     is_active: bool,
//     created_at: Option<chrono::DateTime<chrono::Utc>>,
// }

// // User Events Enum
// #[derive(Debug, Clone, PartialEq, Serialize, DeserializeOwned)]
// pub enum UserEvent {
//     UserCreated { email: String, timestamp: chrono::DateTime<chrono::Utc> },
//     UserActivated,
// }
// impl Message for UserEvent {}
// impl DomainEvent for UserEvent {}

// // State evolution logic
// impl AggregateState for UserState {
//     type Event = UserEvent;
//     fn apply(&mut self, event: &Self::Event) {
//         match event {
//             UserEvent::UserCreated { email, timestamp } => {
//                 if self.created_at.is_none() { // Idempotency check (optional but good)
//                     self.email = Some(email.clone());
//                     self.created_at = Some(*timestamp);
//                     self.is_active = false;
//                 }
//             }
//             UserEvent::UserActivated => {
//                 self.is_active = true;
//             }
//         }
//     }
// }

// // Example Command, Error, Logic (Create User)
// #[derive(Debug)] pub struct CreateUserCmd { pub user_id: String, pub email: String }
// impl Message for CreateUserCmd {}
// #[derive(Error, Debug)] #[error("User already exists: {0}")] pub struct UserExistsError(String);
// impl Command for CreateUserCmd { type Result = (); type Error = UserExistsError; }

// pub struct CreateUserLogic; // The logic handler struct
// pub struct UserServices { /* Dependencies like email checker */ } // Placeholder for services

// impl AggregateCommandHandler<UserState, CreateUserCmd, UserServices> for CreateUserLogic {
//     fn handle(&self, state: &UserState, command: CreateUserCmd, _services: &UserServices)
//         -> Result<Vec<UserEvent>, UserExistsError>
//     {
//         if state.created_at.is_some() { // Check business rule using current state
//             Err(UserExistsError(command.user_id))
//         } else {
//             // Potentially use _services here
//             Ok(vec![UserEvent::UserCreated {
//                 email: command.email,
//                 timestamp: chrono::Utc::now(), // Generate event data
//             }])
//         }
//     }
//     fn derive_result(&self) -> <CreateUserCmd as Command>::Result { () } // Simple Ack
// }
