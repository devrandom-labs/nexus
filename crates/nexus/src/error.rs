use std::error::Error as StdError;
use thiserror::Error as ThisError;
use tower::BoxError;

#[derive(Debug, ThisError)]
pub enum Error<H>
where
    H: StdError + Send + Sync + 'static,
{
    #[error("Handler not found for message: {0:?}")]
    HandlerNotFound(String),

    #[error("Internal dispater error")]
    InternalError(#[source] BoxError),

    #[error("Handler execution failed")]
    HandlerFailed(#[from] H),
}

#[derive(Debug, ThisError)]
#[error("Registration Failed: {0}")]
pub struct RegistrationFailed(pub String);
