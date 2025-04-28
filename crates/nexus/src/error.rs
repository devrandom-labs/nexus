use std::{any::TypeId, error::Error as StdError};
use thiserror::Error as ThisError;
use tower::BoxError;

#[derive(Debug, ThisError)]
pub enum Error<H>
where
    H: StdError + Send + Sync + 'static,
{
    #[error("Handler not found for type ID {0:?}")]
    HandlerNotFound(TypeId),

    #[error("Internal dispater error")]
    InternalError(#[source] BoxError),

    #[error("Handler execution failed")]
    HandlerFailed(#[from] H),
}
