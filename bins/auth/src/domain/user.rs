#![allow(dead_code)]
use super::email::Email;
use super::hashed_password::HashedPassword;
use super::user_id::UserId;

#[derive(Debug)]
pub enum User {
    Unverified {
        email: Email,
    },
    Verified {
        email: Email,
    },
    Active {
        id: UserId,
        email: Email,
        hashed_password: HashedPassword,
    },
}
