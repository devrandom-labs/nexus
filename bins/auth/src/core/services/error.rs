#![allow(dead_code)]
use password_hash::errors::Error as PasswordHashError;
use thiserror::Error as TError;

#[derive(Debug, Clone, TError, PartialEq)]
pub enum Error {
    #[error("{0}")]
    PasswordHash(PasswordHashError),
}
