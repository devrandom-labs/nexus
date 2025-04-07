#![allow(dead_code)]
use super::password_validator::PasswordValidationErrors;
use thiserror::Error as TError;

#[derive(Debug, Clone, TError, PartialEq)]
pub enum Error {
    #[error("{}", .0)]
    PasswordValidation(PasswordValidationErrors),
}
