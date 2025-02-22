#![allow(dead_code)]
use std::fmt::Debug;
use std::future::Future;
use thiserror::Error as Err;
use tokio::sync::mpsc::channel;

pub mod context;
pub mod message;
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
// bus also takes the context that is provided to the handler execute function
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

    pub async fn start<M, F, Fut, R>(
        &self,
        handler: F, // TODO: change handler to tower service.
    ) -> Result<message_sender::MessageSender<M, R>, Error>
    where
        R: Message + Debug + Send + Sync + 'static,
        M: Message + Send + Sync + 'static + Clone,
        F: Fn(M) -> Fut + Clone + Sync + Send + 'static,
        Fut: Future<Output = MessageResult<R>> + Send + Sync + 'static, // TODO: change handler to tower service.
    {
        let (tx, mut rx) = channel(self.bound);
        let sender = message_sender::MessageSender::<M, R>::new(tx);

        tokio::spawn(async move {
            while let Some(msg_env) = rx.recv().await {
                async move {
                    let msg = msg_env.message();
                    let response = handler(msg).await;
                    let _ = msg_env.reply(response);
                }
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
        HeiHei,
    }

    impl Message for BusMessage {
        fn get_type(&self) -> String {
            match self {
                BusMessage::Hello => "hello".to_string(),
                BusMessage::HeiHei => "heihei".to_string(),
            }
        }

        fn get_version(&self) -> String {
            "0".to_string()
        }
    }

    #[derive(Debug, PartialEq, Clone)]
    enum MessageResponse {
        HelloResponse,
        HeiHeiResponse,
    }

    impl Message for MessageResponse {
        fn get_type(&self) -> String {
            match self {
                Self::HelloResponse => "hello response".to_string(),
                Self::HeiHeiResponse => "hei hei response".to_string(),
            }
        }

        fn get_version(&self) -> String {
            "0".to_string()
        }
    }

    #[tokio::test]
    async fn should_be_able_to_take_handlers() {
        let bus = Bus::new(20);
        // now response really needs to just be Dyn Serializeable
        let sender = bus
            .start(|msg: BusMessage| async move {
                match msg {
                    BusMessage::Hello => Ok(MessageResponse::HelloResponse),
                    BusMessage::HeiHei => Ok(MessageResponse::HeiHeiResponse),
                    _ => Err(Error::ReplyFailed),
                };
            })
            .await
            .unwrap();

        let reply = sender.send(BusMessage::Hello).await;
        assert!(reply.is_ok());
        let result = reply.unwrap();
        assert_eq!(result, MessageResponse::HelloResponse)
    }

    #[tokio::test]
    async fn should_be_able_to_work_between_tasks() {
        let bus = Bus::new(20);
        // now response really needs to just be Dyn Serializeable
        let sender = bus
            .start(|msg: BusMessage| async move {
                match msg {
                    BusMessage::Hello => Ok(MessageResponse::HelloResponse),
                    BusMessage::HeiHei => Ok(MessageResponse::HeiHeiResponse),
                    _ => Err(Error::ReplyFailed),
                };
            })
            .await
            .unwrap();

        tokio::spawn({
            let sender = sender.clone();
            async move {
                let reply = sender.send(BusMessage::Hello).await;
                assert!(reply.is_ok());
                let result = reply.unwrap();
                assert_eq!(result, MessageResponse::HelloResponse)
            }
        });

        tokio::spawn({
            let sender = sender.clone();
            async move {
                let reply = sender.send(BusMessage::HeiHei).await;
                assert!(reply.is_ok());
                let result = reply.unwrap();
                assert_eq!(result, MessageResponse::HeiHeiResponse)
            }
        });
    }
}

// TODO: messages should be serialized and deserialized while getting processed.
// TODO: since we do not care platform specific serialization, we need quick, efficient serialization
