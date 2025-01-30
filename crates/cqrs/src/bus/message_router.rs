use super::{Message, MessageResponse, MessageResult};
use std::future::Future;
use std::marker::PhantomData;

// not cloning this , this would be used in the thread where message handler would be executed
pub struct MessageRouter<M, F, Fut, R>
where
    M: Message,
    R: MessageResponse,
    F: FnMut(M) -> Fut,
    Fut: Future<Output = MessageResult<R>>,
{
    router: Box<F>,
    _message: PhantomData<M>,
}

impl<M, F, Fut, R> MessageRouter<M, F, Fut, R>
where
    M: Message,
    R: MessageResponse,
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
