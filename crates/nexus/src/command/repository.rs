use super::aggregate::{AggregateLoadError, AggregateRoot, AggregateType as AT};
use crate::Id as AggregateId;
use std::{error::Error as StdError, fmt::Debug, future::Future, pin::Pin};
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
    AggregateLoadError<Id>: StdError + Send + Sync + 'static,
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

    /// ## Variant: `DataIntegrityError`
    /// Indicates an issue with the integrity of the loaded event data for a specific aggregate,
    /// such as events with mismatched aggregate IDs.
    /// The `source` is an [`AggregateLoadError<Id>`].
    #[error("Loaded data integrity issue for aggregator '{aggregate_id:?}'")]
    DataIntegrityError {
        aggregate_id: Id,
        #[source]
        source: AggregateLoadError<Id>,
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

/// # `EventSourceRepository`
///
/// A trait (Port in Hexagonal Architecture) defining the contract for loading and saving
/// event-sourced aggregates (`AggregateRoot<Self::AggregateType>`).
///
/// Implementations of this trait (Adapters) will provide the concrete logic for
/// interacting with a specific event store technology (e.g., a relational database,
/// a dedicated event store like EventStoreDB, or an in-memory store for testing).
///
/// This trait must be `Send + Sync + Clone + Debug` to ensure it can be shared
/// across threads and used in various contexts. `Clone` is often useful for sharing
/// repository instances (e.g., if they are lightweight handles to a connection pool).
pub trait EventSourceRepository: Send + Sync + Clone + Debug {
    /// ## Associated Type: `AggregateType`
    /// Specifies the kind of aggregate (which must implement [`AT`](super::aggregate::AggregateType))
    /// this repository can manage. This links the repository to the aggregate's
    /// specific `Id`, `Event`, and `State` types.
    type AggregateType: AT;

    /// ## Method: `load`
    /// Asynchronously loads an [`AggregateRoot`] instance by its unique `id`.
    ///
    /// Implementations should:
    /// 1.  Retrieve the sequence of domain events for the given `id` from the event store.
    /// 2.  Rehydrate the aggregate by calling [`AggregateRoot::load_from_history`].
    ///
    /// ### Parameters:
    /// * `id`: A reference to the ID of the aggregate to load.
    ///
    /// ### Returns:
    /// A `Pin<Box<dyn Future<Output = Result<AggregateRoot<Self::AggregateType>, RepositoryError<...>>> + Send + 'a>>`.
    /// This signifies an asynchronous operation that will eventually produce:
    /// * `Ok(AggregateRoot<Self::AggregateType>)`: If the aggregate is found and successfully loaded.
    /// * `Err(RepositoryError<Id>)`: If the aggregate is not found, or if any other
    ///   error occurs during loading (e.g., deserialization issues, store errors).
    ///
    /// The `'a` lifetime ensures the future does not outlive the `id` reference.
    #[allow(clippy::type_complexity)]
    fn load(
        &self,
        id: &<Self::AggregateType as AT>::Id,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AT>::Id>,
                    >,
                > + Send
                + 'static,
        >,
    >;

    /// ## Method: `save`
    /// Asynchronously saves an [`AggregateRoot`] instance.
    ///
    /// Implementations should:
    /// 1.  Take the uncommitted events from the aggregate using `aggregate.take_uncommitted_events()`.
    /// 2.  Atomically append these new events to the event stream for the aggregate in the event store.
    /// 3.  Typically, this involves an optimistic concurrency check using the aggregate's
    ///     `version()` or `current_version()` against the current stream version in the store.
    ///
    /// ### Parameters:
    /// * `aggregate`: The `AggregateRoot` instance to save. It is passed by value,
    ///   implying the repository might consume it or parts of it (like its uncommitted events).
    ///   Consider if `&mut AggregateRoot<Self::AggregateType>` would be more appropriate
    ///   if the aggregate root itself needs to be updated post-save (e.g., version incremented
    ///   and uncommitted events cleared, though `take_uncommitted_events` already handles clearing).
    ///   However, passing by value is common if the aggregate is conceptually "done" after this.
    ///
    /// ### Returns:
    /// A `Pin<Box<dyn Future<Output = Result<(), RepositoryError<...>>> + Send + 'a>>`.
    /// This signifies an asynchronous operation that will eventually produce:
    /// * `Ok(())`: If the aggregate's events are successfully saved.
    /// * `Err(RepositoryError<Id>)`: If any error occurs during saving (e.g.,
    ///   serialization issues, store errors, concurrency conflicts).
    ///
    /// The `'a` lifetime here relates to `&'a self`.
    #[allow(clippy::type_complexity)]
    fn save(
        &self,
        aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(), RepositoryError<<Self::AggregateType as AT>::Id>>>
                + Send
                + 'static,
        >,
    >;
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn should_load_aggregate_when_correct_id_is_provided() {}

    #[tokio::test]
    async fn should_give_aggregate_not_found_error_when_invalid_id_is_provided() {}

    #[tokio::test]
    async fn should_save_aggregate_uncommited_events() {}

    #[tokio::test]
    async fn should_give_conflict_error_when_version_mismatch_while_saving_aggregates() {} // optimistic locking

    #[tokio::test]
    async fn should_give_data_integrity_error_if_aggrgate_id_mismatches_with_event_aggregate_id_on_load()
     {
    }

    #[tokio::test]
    async fn should_give_store_error_on_unreleated_database_error() {}

    #[tokio::test]
    async fn should_return_deserialization_error_on_load() {}

    #[tokio::test]
    async fn should_return_serialization_error_on_save() {}
}
