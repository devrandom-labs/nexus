#![allow(dead_code)]
use std::collections::HashMap;
use std::future::Future;
use ulid::Ulid;

pub struct MessageTypeId(Ulid);

impl MessageTypeId {
    pub fn new() -> Self {
        MessageTypeId(Ulid::new())
    }
}

pub trait Message {
    fn type_id(&self) -> MessageTypeId;
}

pub trait MessageHandler: Message {
    type Response;
    type Error;
    type Future: Future<Output = Result<Self::Response, Self::Error>>;
    fn execute(self) -> Self::Future;
}

#[cfg(test)]
mod test {
    use super::*;
    use std::pin::Pin;

    struct Test(String);

    // each message should have a unique id.
    impl Message for Test {
        fn type_id(&self) -> MessageTypeId {
            MessageTypeId::new()
        }
    }

    impl MessageHandler for Test {
        type Response = String;
        type Error = String;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;
        fn execute(self) -> Self::Future {
            Box::pin(async { Ok(self.0) })
        }
    }

    #[tokio::test]
    async fn message_handler_should_be_a_future() {
        let test_message = Test("Test Message".to_string());
        let reply = test_message.execute().await;
        assert!(reply.is_ok());
        let reply = reply.unwrap();
        assert_eq!(reply, "Test Message".to_string());
    }
}
