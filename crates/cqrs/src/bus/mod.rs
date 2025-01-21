#![allow(dead_code)]

pub mod error;
pub mod message;
pub mod message_sender;

#[derive(Debug)]
pub struct Bus {
    bound: usize,
}

impl Bus {
    pub fn new(bound: usize) -> Self {
        Bus { bound }
    }
}
