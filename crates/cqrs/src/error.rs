use std::error::Error as StdError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AggregateError<T: StdError> {
    #[error("{0}")]
    UserError(T),
}
