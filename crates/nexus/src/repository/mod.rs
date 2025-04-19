use crate::aggregate::{AggregateRoot, AggregateType, aggregate_root::AggregateLoadError};
use std::{error::Error as StdError, fmt::Debug, future::Future, hash::Hash, pin::Pin};
use thiserror::Error as ThisError;
use tower::BoxError;

#[derive(Debug, ThisError)]
pub enum RepositoryError<Id>
where
    Id: Debug + Send + Sync + Hash + Eq + Clone + 'static,
    AggregateLoadError<Id>: StdError + Send + Sync + 'static,
{
    #[error("Aggregate with ID '{0:?}' not found")]
    AggregateNotFound(Id),

    #[error("Failed to deserialize event data from store")]
    DeserializationError {
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
}

pub trait EventSourceRepository: Send + Sync + Clone {
    type AggregateType: AggregateType;

    #[allow(clippy::type_complexity)]
    fn load<'a>(
        &'a self,
        id: &'a <Self::AggregateType as AggregateType>::Id,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    >;
}
