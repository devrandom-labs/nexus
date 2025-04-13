#![allow(dead_code)]

#[derive(Debug)]
pub struct Email(String);

impl Email {
    pub fn new(email: String) -> Self {
        Email(email)
    }
}
