#![allow(dead_code)]
use super::email::Email;
use super::password::Password;
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
        password: Password,
    },
}

pub trait Aggregate {
    type Root;
    type Events;
    type Error: std::error::Error;
    fn apply(state: Option<Self::Root>, events: &Self::Events) -> Result<Self::Root, Self::Error>;
}
