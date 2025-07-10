use crate::domain::{AggregateRoot, AggregateType as AT, Id as AggregateId};
use async_trait::async_trait;
use std::fmt::Debug;
use thiserror::Error as ThisError;
use tower::BoxError;

/// # `RepositoryError<Id>`
///
/// Defines errors that can occur when interacting with an [`EventSourceRepository`].
///
/// This enum covers issues like not finding an aggregate, problems with data
/// serialization/deserialization, underlying store errors, data integrity issues
/// during loading, and optimistic concurrency conflicts.
///
/// ## Type Parameters:
/// * `Id`: The type of the aggregate's identifier. It must be `Debug + Send + Sync + Hash + Eq + Clone + 'static`.
///   The `AggregateLoadError<Id>` (which is a source for `DataIntegrityError`) must also
///   be a standard error that is `Send + Sync + 'static`.
#[derive(Debug, ThisError)]
pub enum RepositoryError<Id>
where
    Id: AggregateId,
    // This bound ensures that AggregateLoadError can be a source in a `thiserror::Error` context.
{
    /// ## Variant: `AggregateNotFound`
    /// Indicates that an aggregate with the specified `Id` could not be found in the store.
    #[error("Aggregate with ID '{0:?}' not found")]
    AggregateNotFound(Id),

    /// ## Variant: `DeserializationError`
    /// Occurs if there's an error deserializing event data retrieved from the event store.
    /// The `source` field contains the underlying error, typically from a serialization library.
    #[error("Failed to deserialize event data from store")]
    DeserializationError {
        #[source]
        source: BoxError,
    },

    /// ## Variant: `SerializationError`
    /// Occurs if there's an error serializing event data for persistence into the event store.
    /// The `source` field contains the underlying error.
    #[error("Failed to serialize event data for store")] // Added for save
    SerializationError {
        #[source]
        source: BoxError,
    },

    /// ## Variant: `StoreError`
    /// Represents an error from the underlying event store itself (e.g., connection issues,
    /// I/O errors, database specific errors not covered by other variants).
    /// The `source` field contains the underlying store-specific error.
    #[error("Failed to read from event store")]
    StoreError {
        #[source]
        source: BoxError,
    },

    /// ## Variant: `Conflict`
    /// Signals an optimistic concurrency control violation. This typically happens when
    /// attempting to save an aggregate, and its expected version (based on when it was loaded)
    /// does not match the current version in the event store, indicating that another
    /// process has modified the aggregate in the meantime.
    #[error(
        "Concurrency conflict for aggregate '{aggregate_id:?}'. Expected version {expected_version}."
    )]
    Conflict {
        aggregate_id: Id,
        expected_version: u64,
    },
}

#[async_trait]
pub trait EventSourceRepository: Send + Sync + Clone + Debug {
    type AggregateType: AT;

    async fn load(
        &self,
        id: &<Self::AggregateType as AT>::Id,
    ) -> Result<AggregateRoot<Self::AggregateType>, RepositoryError<<Self::AggregateType as AT>::Id>>;

    async fn save(
        &self,
        aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Result<(), RepositoryError<<Self::AggregateType as AT>::Id>>;
}
