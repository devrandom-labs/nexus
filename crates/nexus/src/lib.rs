use serde::{Serialize, de::DeserializeOwned};
use std::{error::Error, fmt::Debug, hash::Hash};

pub mod command;
pub mod error;
pub mod query;

// Common marker trait for all message types in our system.
pub trait Message: Debug + Send + Sync + 'static {}

/// Represents an intention to change the system state
/// It is linked to specific Result and Error types
pub trait Command: Message {
    /// The type returned upon successful processing of the command.
    type Result: Send + Sync + Debug + 'static;

    /// The specific error type returned if the command's *domain logic* fails.
    /// This is distinct  from infrastructure errors (like database connection issues).
    type Error: Error + Send + Sync + Debug + 'static;
}

/// Represents an intention to retreive data from the system without changing state.
/// It is linked to specific Result (the data requested) and Error types.
pub trait Query: Message {
    /// The type of data returned upon successful execution of the query.
    /// This is typically a specific struct or enum representing the read model data.
    type Result: Send + Sync + Debug + 'static;
    /// The specific error type returned if the query fails (e.g., data not found, access denied).
    type Error: Error + Send + Sync + Debug + 'static;
}

/// Represents a significant occurrence in the domain that has already happened.
/// Events are immutable facts.
pub trait DomainEvent: Message + Clone + Serialize + DeserializeOwned {
    type Id: Clone + Send + Sync + Debug + Hash + Eq + 'static;
    fn aggregate_id(&self) -> &Self::Id;
}
