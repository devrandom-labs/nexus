use crate::aggregate::{AggregateRoot, AggregateType};
use std::{fmt::Debug, future::Future, pin::Pin};
use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum RepositoryError<Id>
where
    Id: Debug,
{
    #[error("Aggregate with ID '{0:?}' not found")]
    AggregateNotFound(Id),
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
