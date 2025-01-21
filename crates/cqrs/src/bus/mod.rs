#![allow(dead_code)]

pub mod error;
pub mod message;
pub mod message_sender;

pub struct Bus {
    bound: usize,
}
