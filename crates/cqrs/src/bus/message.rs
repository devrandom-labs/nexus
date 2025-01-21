#![allow(dead_code)]

use super::error::Error;
use std::fmt::Debug;
use tokio::sync::oneshot::Sender;

pub type MessageResult<R> = Result<R, Error>;
pub type Reply<R> = Sender<MessageResult<R>>;

/// payload should not be dynamic dispatch,
/// sum type of messages should be sent through the messages,
///
/// but reply can transfer any kind of type back, so it can be dynamic dispatch
#[derive(Debug)]
pub struct MessageEnvelope<T, R>
where
    R: Debug,
    T: Debug,
{
    reply: Reply<R>,
    message: T,
}

impl<T, R> MessageEnvelope<T, R>
where
    R: Debug,
    T: Debug,
{
    pub fn new(reply: Reply<R>, message: T) -> Self {
        MessageEnvelope { reply, message }
    }
}
