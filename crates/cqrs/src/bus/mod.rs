#![allow(dead_code)]
use std::fmt::Debug;
use thiserror::Error as Err;
use tokio::sync::oneshot::Sender;

pub mod message_envelope;
pub mod message_router;
pub mod message_sender;

#[allow(clippy::enum_variant_names)]
#[derive(Err, Debug)]
pub enum Error {
    #[error("Message send failed")]
    SendingFailed,
    #[error("Handler execution failed")]
    HandlerExecutionFailed,
    #[error("Failed to reply")]
    ReplyFailed,
}

pub type MessageResult<R> = Result<R, Error>;

// Every reply should send this message.
pub type Reply<R> = Sender<MessageResult<R>>;

pub trait Message {
    fn get_version(&self) -> String;
    fn get_type(&self) -> String;
}

// takes an enum of messages handlers can process
// sends out trait object of message response [which can derive serialize / deserialize, depends]
