#![allow(dead_code)]
use ulid::Ulid;

#[derive(Debug)]
pub struct UserId(Ulid);
