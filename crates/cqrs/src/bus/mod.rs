#![allow(dead_code)]
use std::fmt::Debug;
use std::future::Future;
use thiserror::Error as Err;
use tokio::sync::{
    mpsc::{channel, Sender as MpscSender},
    oneshot::Sender,
};
use tokio::task;

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
    #[error("Failed to start message bus")]
    BusStartFailed,
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
pub trait MessageResponse {}

pub struct Bus {
    bound: usize,
}
impl Bus {
    pub fn new(bound: usize) -> Self {
        // TODO: should there be a cutoff, uppwer bound?
        // TODO: should there be a lower bound?
        Bus { bound }
    }

    // should return serializeable.
    pub async fn start<M, F, Fut, R>(
        &self,
        handler: F,
    ) -> Result<message_sender::MessageSender<M, R>, Error>
    where
        M: Message,
        F: FnMut(M) -> Fut,
        Fut: Future<Output = MessageResult<R>>,
    {
        let (tx, mut rx) = channel(self.bound);
        let sender = message_sender::MessageSender::<M, R>::new(tx);
        task::spawn(async move {});
        Ok(sender)
    }
}
