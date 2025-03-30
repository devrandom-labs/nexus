#![allow(dead_code)]
use super::email::Email;
use super::password::Password;
use super::user_id::UserId;

#[derive(Debug)]
pub struct User {
    user_id: UserId,
    email: Email,
    password: Password,
}

// TODO: add states to aggregate, pending, active
