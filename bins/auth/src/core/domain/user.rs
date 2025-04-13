#![allow(dead_code)]
use super::email::Email;
use super::user_id::UserId;
use password_hash::PasswordHashString;

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
        password_hash: PasswordHashString,
    },
}
