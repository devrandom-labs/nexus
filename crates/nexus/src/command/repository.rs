use super::aggregate::{AggregateLoadError, AggregateRoot, AggregateType as AT};
use crate::Id as AggregateId;
use std::{error::Error as StdError, fmt::Debug, future::Future, pin::Pin};
use thiserror::Error as ThisError;
use tower::BoxError;

#[derive(Debug, ThisError)]
pub enum RepositoryError<Id>
where
    Id: AggregateId,
    AggregateLoadError<Id>: StdError + Send + Sync + 'static,
{
    #[error("Aggregate with ID '{0:?}' not found")]
    AggregateNotFound(Id),

    #[error("Failed to deserialize event data from store")]
    DeserializationError {
        #[source]
        source: BoxError,
    },

    #[error("Failed to serialize event data for store")] // Added for save
    SerializationError {
        #[source]
        source: BoxError,
    },

    #[error("Failed to read from event store")]
    StoreError {
        #[source]
        source: BoxError,
    },

    #[error("Loaded data integrity issue for aggregator '{aggregate_id:?}'")]
    DataIntegrityError {
        aggregate_id: Id,
        #[source]
        source: AggregateLoadError<Id>,
    },

    #[error(
        "Concurrency conflict for aggregate '{aggregate_id:?}'. Expected version {expected_version}."
    )]
    Conflict {
        aggregate_id: Id,
        expected_version: u64,
    },
}

pub trait EventSourceRepository: Send + Sync + Clone + Debug {
    type AggregateType: AT;

    #[allow(clippy::type_complexity)]
    fn load<'a>(
        &'a self,
        id: &'a <Self::AggregateType as AT>::Id,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AT>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    >;

    #[allow(clippy::type_complexity)]
    fn save<'a>(
        &'a self,
        aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(), RepositoryError<<Self::AggregateType as AT>::Id>>>
                + Send
                + 'a,
        >,
    >;
}
