use crate::application::ApplicationError;
use thiserror::Error as TError;

#[derive(Debug, TError)]
pub enum Error {
    #[error("{0}")]
    Application(#[from] ApplicationError),
}
