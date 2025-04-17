#![allow(dead_code)]
use std::fmt::Display;

#[derive(Debug)]
pub struct Email(String);

impl Email {
    pub fn new(email: String) -> Self {
        Email(email)
    }
}

impl Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "email: {}", &self.0)
    }
}
