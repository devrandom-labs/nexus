use super::{Message, MessageResult};
use std::future::Future;
use std::marker::PhantomData;

// not cloning this , this would be used in the thread where message handler would be executed
pub struct MessageRouter<M, F, Fut, R>
where
    M: Message,
    F: FnMut(M) -> Fut,
    Fut: Future<Output = MessageResult<R>>,
{
    router: Box<F>,
    _message: PhantomData<M>,
}

impl<M, F, Fut, R> MessageRouter<M, F, Fut, R>
where
    M: Message,
    F: FnMut(M) -> Fut,
    Fut: Future<Output = MessageResult<R>>,
{
    pub fn new(router: F) -> Self {
        let router = Box::new(router);
        MessageRouter {
            router,
            _message: PhantomData,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::bus::Error;

    pub enum Messages {
        SomeMessage { content: String },
    }

    impl Message for Messages {
        fn get_type(&self) -> String {
            match *self {
                Self::SomeMessage { .. } => String::from("Some message"),
            }
        }

        fn get_version(&self) -> String {
            "1".to_string()
        }
    }

    pub struct SomeMessageResponse {
        reply: String,
    }

    async fn router(messages: Messages) -> Result<SomeMessageResponse, Error> {
        match messages {
            Messages::SomeMessage { content } => Ok(SomeMessageResponse { reply: content }),
        }
    }

    #[tokio::test]
    async fn should_take_async_closure() {
        let router = MessageRouter::new(router);
        let message = Messages::SomeMessage {
            content: "hello".to_string(),
        };
        let _response = (router.router)(message).await;
    }
}
