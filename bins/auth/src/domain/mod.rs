use thiserror::Error as TError;

pub(super) mod email;
pub mod hashed_password;
pub(super) mod password;
pub(super) mod user;
pub(super) mod user_id;

#[derive(Debug, TError)]
pub enum Error {}
