/// # `Query`
///
/// Represents an intention to retrieve data from the system without changing its state.
/// Queries are messages that ask the system for information.
///
/// In a CQRS architecture, queries are handled separately from commands, often
/// accessing specialized read models that are optimized for data retrieval.
///
/// This trait requires `Message` and associates specific `Result` (the data being
/// requested) and `Error` types with each query, defining a clear contract for
/// query execution.
///
use super::message::Message;
use std::{error::Error, fmt::Debug};

pub trait Query: Message {
    /// ## Associated Type: `Result`
    /// The type of data returned upon successful execution of the query.
    ///
    /// This is typically a specific struct or enum representing the read model data
    /// requested by the query (e.g., `UserDetailsView`, `OrderSummaryDto`).
    ///
    /// It must be `Send + Sync + Debug + 'static`.
    type Result: Send + Sync + Debug + 'static;
    /// ## Associated Type: `Error`
    /// The specific error type returned if the query fails.
    ///
    /// This could be due to various reasons such as the requested data not being
    /// found, the requester lacking necessary permissions, or issues with the
    /// underlying read model store.
    ///
    /// It must implement `std::error::Error` and be `Send + Sync + Debug + 'static`.
    type Error: Error + Send + Sync + Debug + 'static;
}
