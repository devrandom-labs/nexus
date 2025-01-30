#![allow(dead_code)]
use std::fmt::Debug;
use std::future::Future;
use thiserror::Error as Err;
use tokio::sync::mpsc::channel;
use tokio::task;

pub mod message_envelope;
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
    const DEFAULT_BOUND: usize = 10;
    const MAX_BOUND: usize = 1000;
    pub fn new(bound: usize) -> Self {
        let bounded_size = match bound {
            0 => Self::DEFAULT_BOUND,
            b if b > Self::MAX_BOUND => Self::MAX_BOUND,
            _ => bound,
        };
        Bus {
            bound: bounded_size,
        }
    }

    // should return serializeable.
    // change handler to services, takes tower services.
    pub async fn start<M, F, Fut, R>(
        &self,
        mut handler: F,
    ) -> Result<message_sender::MessageSender<M, R>, Error>
    where
        R: Send + Sync + 'static + Debug, // takes only one type of reply, should be able to take dyn MessageReplys
        M: Message + Send + Sync + 'static,
        F: FnMut(&M) -> Fut + Send + 'static,
        Fut: Future<Output = MessageResult<R>> + Send + Sync,
    {
        let (tx, mut rx) = channel(self.bound);
        let sender = message_sender::MessageSender::<M, R>::new(tx);
        task::spawn(async move {
            while let Some(msg_env) = rx.recv().await {
                let resp = handler(msg_env.message()).await;
                // have to handle the errors here.
                let _ = msg_env.reply(resp);
            }
        });
        Ok(sender)
    }
}
