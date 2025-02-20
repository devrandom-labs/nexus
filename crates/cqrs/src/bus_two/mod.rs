#![allow(dead_code)]
use std::any::{Any, TypeId};
pub trait Message: Any + Send + Sync + 'static {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}
//-------------------- error --------------------//
use thiserror::Error as Err;
#[derive(Debug, Err, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusError {}

//-------------------- handler --------------------//
use tower::Service;

pub trait MessageHandler<M: Message>:
    Service<M, Response = (), Error = BusError> + Send + Sync + 'static
{
}

//-------------------- tests --------------------//
#[cfg(test)]
mod test {
    use super::Message;
    use std::any::TypeId;

    #[test]
    fn test_message_impl() {
        struct TestMessage(String);
        impl Message for TestMessage {}
        let s = TestMessage("hello".to_string());
        assert_eq!(s.type_id(), TypeId::of::<TestMessage>()); // on way to check if Message works.
    }
}
