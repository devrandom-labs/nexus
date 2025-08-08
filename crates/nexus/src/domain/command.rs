/// # `Command`
///
/// Represents an intention to change the system state. Commands are imperative
/// messages that instruct the system to perform an action.
///
/// In a CQRS architecture, commands are distinct from queries. They are handled
/// by command handlers (often associated with aggregates in DDD) and typically
/// result in state changes, which are often captured as domain events in an
/// Event Sourcing model.
///
/// This trait requires `Message` and associates specific `Result` and `Error`
/// types with each command, providing a clear contract for command execution.
///
use super::message::Message;
use std::{error::Error, fmt::Debug};

pub trait Command: Message {
    /// ## Associated Type: `Result`
    /// The type returned upon successful processing of the command by its handler.
    ///
    /// This could be a simple acknowledgment (e.g., `()`), an identifier (e.g., the ID
    /// of a newly created aggregate), or any other data relevant to the outcome of
    /// the command.
    ///
    /// It must be `Send + Sync + Debug + 'static`.
    type Result: Send + Sync + Debug + 'static;

    /// ## Associated Type: `Error`
    /// The specific error type returned if the command's *domain logic* fails.
    ///
    /// This error type should represent failures related to business rules, invariants,
    /// or other conditions within the domain that prevent the command from being
    /// successfully processed. It is distinct from infrastructure errors (like
    /// database connection issues or network failures), which would typically be
    /// handled at a different layer (e.g., by the repository or dispatcher).
    ///
    /// It must implement `std::error::Error` and be `Send + Sync + Debug + 'static`.
    type Error: Error + Send + Sync + Debug + 'static;
    fn name(&self) -> &'static str;
}
