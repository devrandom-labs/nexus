use crate::commons::PasswordValidationErrors;
use thiserror::Error as TError;

pub(crate) mod email;
pub(crate) mod password;
pub(crate) mod user;
pub(crate) mod user_id;

#[derive(Debug, TError)]
pub enum Error {
    #[error("{0}")]
    PasswordValidation(#[from] PasswordValidationErrors),
}
