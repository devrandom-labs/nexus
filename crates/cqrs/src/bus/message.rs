#![allow(dead_code)]
use super::error::Error;
use std::fmt::Debug;
use tokio::sync::oneshot::Sender;
use tracing::{error, instrument};

pub type MessageResult<R> = Result<R, Error>;
pub type Reply<R> = Sender<MessageResult<R>>;

/// payload should not be dynamic dispatch,
/// sum type of messages should be sent through the messages,
///
/// but reply can transfer any kind of type back, so it can be dynamic dispatch
#[derive(Debug)]
pub struct MessageEnvelope<T, R> {
    reply: Reply<R>,
    message: T,
}

impl<T, R> MessageEnvelope<T, R> {
    pub fn new(reply: Reply<R>, message: T) -> Self {
        MessageEnvelope { reply, message }
    }

    pub fn message(&self) -> &T {
        &self.message
    }

    #[instrument(skip(self))]
    pub fn reply(self, response: MessageResult<R>) -> Result<(), Error>
    where
        R: Debug,
    {
        self.reply
            .send(response)
            .inspect_err(|err| error!(?err))
            .map_err(|_| Error::ReplyFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot::channel;

    #[derive(Debug)]
    struct TestMessage {
        content: String,
    }

    #[derive(Debug)]
    struct TestReply {
        result: String,
    }

    #[tokio::test]
    async fn construct_and_reply() {
        let (tx, rx) = channel();
        let message = TestMessage {
            content: "Hello".to_string(),
        };
        let message_envelope = MessageEnvelope::new(tx, message);
        let response = TestReply {
            result: "Hei Hei".to_string(),
        };
        let result = message_envelope.reply(Ok(response));
        assert!(result.is_ok());
        let result = tokio::spawn(async move { rx.await.unwrap() }).await;
        assert!(result.is_ok(), "Reply message failed: {:?}", result);
        let response = result.unwrap().unwrap();
        assert_eq!(response.result, "Hei Hei");
    }
}
