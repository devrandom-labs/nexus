#![allow(dead_code)]
use tokio::sync::oneshot::Sender;

pub mod error;
pub mod message_envelope;
pub mod message_handlers;
pub mod message_sender;

pub type MessageResult<R> = Result<R, error::Error>;

// Every reply should send this message.
pub type Reply<R> = Sender<MessageResult<R>>;
