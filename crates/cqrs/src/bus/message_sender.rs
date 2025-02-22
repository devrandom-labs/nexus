#![allow(dead_code)]
use super::message_envelope::MessageEnvelope;
use super::{Error, MessageResult};
use std::clone::Clone;
use std::fmt::Debug;
use tokio::sync::{mpsc::Sender, oneshot};
use tracing::{error, instrument};

type BusSender<T, R> = Sender<MessageEnvelope<T, R>>;

// exector should return a reply back
// this can be used to denote whether the function passed or failed,
#[derive(Clone)]
pub struct MessageSender<T: Clone, R>(Sender<MessageEnvelope<T, R>>);

impl<T, R> MessageSender<T, R>
where
    T: Clone,
{
    pub fn new(sender: BusSender<T, R>) -> Self {
        MessageSender(sender)
    }

    #[instrument(skip(self))]
    pub async fn send(&self, message: T) -> Result<R, Error>
    where
        T: Debug,
        R: Debug,
    {
        let (tx, rx) = oneshot::channel::<MessageResult<R>>();
        let message_envelope = MessageEnvelope::new(tx, message);
        self.0
            .send(message_envelope)
            .await
            .inspect_err(|err| error!(?err))
            .map_err(|_| Error::SendingFailed)?;
        rx.await
            .inspect(|err| error!(?err))
            .map_err(|_| Error::HandlerExecutionFailed)?
    }
}

// TODO: to clone Message sender, we do not need T and R to be cloneable.

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::channel;

    #[derive(Debug, Clone)]
    struct TestMessage {
        content: String,
    }

    #[derive(Debug, Clone)]
    struct TestResponse {
        result: String,
    }

    #[tokio::test]
    async fn send_message_and_receive_reply() {
        let (tx, mut rx) = channel(10);
        let message = TestMessage {
            content: "Hello".to_string(),
        };
        let sender = MessageSender::<TestMessage, TestResponse>::new(tx);

        tokio::spawn(async move {
            if let Some(msg_env) = rx.recv().await {
                let response = TestResponse {
                    result: "Ok".to_string(),
                };
                msg_env
                    .reply(Ok(response))
                    .expect("Failed to send response");
            }
        });

        let result = sender.send(message).await;

        assert!(result.is_ok(), "Sending message failed: {:?}", result);
        let response = result.unwrap();
        assert_eq!(response.result, "Ok");
    }

    // TODO: test sender in multiple tokio tasks
    #[tokio::test]
    async fn send_message_in_different_task() {
        let (tx, mut rx) = channel(10);
        let message = TestMessage {
            content: "Hello".to_string(),
        };
        let sender = MessageSender::<TestMessage, TestResponse>::new(tx);

        tokio::spawn(async move {
            if let Some(msg_env) = rx.recv().await {
                let response = TestResponse {
                    result: "Ok".to_string(),
                };
                msg_env
                    .reply(Ok(response))
                    .expect("Failed to send response");
            }
        });

        tokio::spawn({
            let sender = sender.clone();
            let message = message.clone();
            async move {
                let result = sender.send(message).await;

                assert!(result.is_ok(), "Sending message failed: {:?}", result);
                let response = result.unwrap();
                assert_eq!(response.result, "Ok");
            }
        });

        tokio::spawn({
            let sender = sender.clone();
            let message = message.clone();
            async move {
                let result = sender.send(message).await;

                assert!(result.is_ok(), "Sending message failed: {:?}", result);
                let response = result.unwrap();
                assert_eq!(response.result, "Ok");
            }
        });
    }
    // TODO: test drop receiver
}
