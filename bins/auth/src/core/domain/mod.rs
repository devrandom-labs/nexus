use crate::commons::password_validation::PasswordValidationErrors;
use thiserror::Error as TError;

pub(crate) mod email;
pub(crate) mod password;
pub(crate) mod user;
pub(crate) mod user_id;

pub use email::Email;

#[derive(Debug, TError)]
pub enum Error {
    #[error("{0}")]
    PasswordValidation(#[from] PasswordValidationErrors),
}
