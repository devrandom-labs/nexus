#![allow(dead_code)]
pub mod error;
pub mod handler;
pub mod message;
pub mod message_sender;

pub type MessageHandler<M, E, S, R> = Box<dyn Fn(M, E, S) -> message::MessageResult<R>>;

#[derive(Debug)]
pub struct Bus {
    bound: usize,
}

impl Bus {
    pub fn new(bound: usize) -> Self {
        Bus { bound }
    }
}
