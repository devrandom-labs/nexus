#![allow(dead_code)]
use thiserror::Error as Err;

#[derive(Err, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Error {}

use std::any::{Any, TypeId};
pub trait Message: Any + Send + Sync + 'static {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

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
