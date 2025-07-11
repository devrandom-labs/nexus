use super::model::ReadModel;
use async_trait::async_trait;
use std::{boxed::Box, error::Error as StdError};

/// # `ReadModelRepository`
///
/// A trait (Port in Hexagonal Architecture) defining the contract for retrieving
/// [`ReadModel`] instances.
///
/// Implementations of this trait (Adapters) will provide the concrete logic for
/// fetching read models from a specific data store (e.g., a relational database,
/// a NoSQL document store, a search index, or an in-memory cache).
///
/// This trait requires `Send + Sync + Clone`. `Clone` is useful for sharing
/// repository instances.
#[async_trait]
pub trait ReadModelRepository: Send + Sync + Clone {
    /// ## Associated Type: `Error`
    /// The specific error type that can be returned by repository operations.
    /// Must implement `Send + Sync + Clone + StdError`.
    type Error: Send + Sync + Clone + StdError;

    /// ## Associated Type: `Model`
    /// The type of [`ReadModel`] this repository manages.
    type Model: ReadModel;

    /// ## Method: `get`
    /// Asynchronously retrieves a [`Self::Model`] instance by its unique `id`.
    ///
    /// ### Parameters:
    /// * `id`: A reference to the ID of the read model to retrieve. The type of the ID
    ///   is `<Self::Model as ReadModel>::Id`.
    ///
    /// ### Returns:
    /// A `Pin<Box<dyn Future<Output = Result<Self::Model, Self::Error>> + Send + 'a>>`.
    /// This signifies an asynchronous operation that will eventually produce:
    /// * `Ok(Self::Model)`: If the read model is found.
    /// * `Err(Self::Error)`: If the read model is not found, or if any other error
    ///   occurs during retrieval.
    ///
    /// The `'a` lifetime ensures the future does not outlive the `id` reference.
    async fn get(&self, id: &<Self::Model as ReadModel>::Id) -> Result<Self::Model, Self::Error>;
}
