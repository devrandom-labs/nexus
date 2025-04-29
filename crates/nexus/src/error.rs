use std::error::Error as StdError;
use thiserror::Error as ThisError;
use tower::BoxError;

#[derive(Debug, ThisError)]
pub enum Error<H>
where
    H: StdError + Send + Sync + 'static,
{
    #[error("Could not register service for: {0:?}")]
    RegistrationFailed(String),

    #[error("Handler not found for message: {0:?}")]
    HandlerNotFound(String),

    #[error("Internal dispater error")]
    InternalError(#[source] BoxError),

    #[error("Handler execution failed")]
    HandlerFailed(#[from] H),
}
