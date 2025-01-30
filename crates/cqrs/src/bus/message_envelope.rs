#![allow(dead_code)]
use super::Error;
use super::{MessageResult, Reply};
use std::fmt::Debug;
use tracing::{error, instrument};

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
    use crate::bus::Error;
    use tokio::sync::oneshot::channel;

    #[derive(Debug)]
    struct TestMessage {
        content: String,
    }

    #[derive(Debug, PartialEq)]

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
        assert!(result.is_ok(), "Reply should succeed");
        let received_response = rx.await;
        assert!(received_response.is_ok(), "Receiving reply should succeed");
        let received_response = received_response.unwrap();
        assert!(received_response.is_ok(), "Response should be Ok");
        assert_eq!(
            received_response.unwrap(),
            TestReply {
                result: "Hei Hei".to_string()
            },
            "Response should match"
        )
    }

    #[tokio::test]
    async fn reply_failed_when_receiver_dropped() {
        let (tx, rx) = channel();
        let message = TestMessage {
            content: "Hello".to_string(),
        };

        let message_envelope = MessageEnvelope::new(tx, message);
        let response = TestReply {
            result: "Hei Hei".to_string(),
        };

        drop(rx);
        let reply_result = message_envelope.reply(Ok(response));
        assert!(
            reply_result.is_err(),
            "Reply should fail receiver is dropped"
        );
        assert!(
            matches!(reply_result, Err(Error::ReplyFailed)),
            "Error should be ReplyFailed"
        );
    }
}
