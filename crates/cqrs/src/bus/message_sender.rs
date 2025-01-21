#![allow(dead_code)]

use super::error::Error;
use super::message::{MessageEnvelope, MessageResult};
use std::fmt::Debug;
use tokio::sync::{mpsc::Sender, oneshot};
use tracing::{error, instrument};

type BusSender<T, R> = Sender<MessageEnvelope<T, R>>;

// exector should return a reply back
// this can be used to denote whether the function passed or failed,
#[derive(Clone)]
pub struct MessageSender<T, R>(Sender<MessageEnvelope<T, R>>);

impl<T, R> MessageSender<T, R> {
    pub fn new(sender: BusSender<T, R>) -> Self {
        MessageSender(sender)
    }

    #[instrument(skip(self))]
    pub async fn send(self, message: T) -> Result<R, Error>
    where
        T: Debug,
        R: Debug,
    {
        let (tx, rx) = oneshot::channel::<MessageResult<R>>();
        let message_envelope = MessageEnvelope::new(tx, message);
        let _ = self
            .0
            .send(message_envelope)
            .await
            .inspect_err(|err| error!(?err))
            .map_err(|_| Error::SendingFailed)?;
        rx.await
            .inspect(|err| error!(?err))
            .map_err(|_| Error::HandlerExecutionFailed)?
    }
}

/// testing
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::message::MessageEnvelope;
    use tokio::sync::mpsc::Sender;

    #[test]
    fn able_to_send_message() {}
    // figure out a way to pass any struct as message
    // no need for debug trait,
    // maybe figure out how I can involve tracing as a drop in config
}
