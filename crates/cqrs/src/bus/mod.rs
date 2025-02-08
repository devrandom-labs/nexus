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

mod test {
    use super::*;
    #[derive(Debug, Clone)]
    enum BusMessage {
        Hello,
    }

    impl Message for BusMessage {
        fn get_type(&self) -> String {
            match self {
                BusMessage::Hello => "hello".to_string(),
            }
        }

        fn get_version(&self) -> String {
            "0".to_string()
        }
    }

    #[derive(Debug, PartialEq, Clone)]
    struct HelloResponse {
        content: String,
    }

    #[derive(Debug, PartialEq)]
    struct HeiHeiResponse {
        content: String,
    }

    #[tokio::test]
    async fn should_be_able_to_take_handlers() {
        let bus = Bus::new(20);
        // now response really needs to just be Dyn Serializeable
        let sender = bus
            .start(|msg: &BusMessage| async move {
                match msg {
                    BusMessage::Hello => Ok(HelloResponse {
                        content: "whats up".to_string(),
                    }),
                }
            })
            .await
            .unwrap();

        let reply = sender.send(BusMessage::Hello).await;
        assert!(reply.is_ok());
        let result = reply.unwrap();
        assert_eq!(
            result,
            HelloResponse {
                content: "whats up".to_string()
            }
        )
    }

    #[tokio::test]
    async fn should_be_able_to_work_between_tasks() {
        let bus = Bus::new(20);
        // now response really needs to just be Dyn Serializeable
        let sender = bus
            .start(|msg: &BusMessage| async move {
                match msg {
                    BusMessage::Hello => Ok(HelloResponse {
                        content: "whats up".to_string(),
                    }),
                }
            })
            .await
            .unwrap();

        tokio::spawn({
            let sender = sender.clone();
            async move {
                let reply = sender.send(BusMessage::Hello).await;
                assert!(reply.is_ok());
                let result = reply.unwrap();
                assert_eq!(
                    result,
                    HelloResponse {
                        content: "whats up".to_string()
                    }
                )
            }
        });

        tokio::spawn({
            let sender = sender.clone();
            async move {
                let reply = sender.send(BusMessage::Hello).await;
                assert!(reply.is_ok());
                let result = reply.unwrap();
                assert_eq!(
                    result,
                    HelloResponse {
                        content: "whats up".to_string()
                    }
                )
            }
        });
    }
}
