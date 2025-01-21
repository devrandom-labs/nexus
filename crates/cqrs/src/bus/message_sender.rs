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
pub struct MessageSender<T: Debug, R: Debug>(Sender<MessageEnvelope<T, R>>);

impl<T, R> MessageSender<T, R>
where
    T: Debug,
    R: Debug,
{
    pub fn new(sender: BusSender<T, R>) -> Self {
        MessageSender(sender)
    }

    #[instrument(skip(self))]
    pub async fn send(self, message: T) -> Result<R, Error> {
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
