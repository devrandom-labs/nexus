use super::body::Body;
use std::any::{Any, TypeId};

// TODO: add time_stamp with instant for tracing feature later
pub struct Message {
    body: Body,
}

impl Message {
    pub fn new<T>(body: T) -> Self
    where
        T: Any + Send + Sync,
    {
        let body = Body::new(body);
        Message { body }
    }

    pub fn type_id(&self) -> TypeId {
        self.body.type_id()
    }

    pub fn get_body_ref(&self) -> &Body {
        &self.body
    }

    pub fn get_body_mut(&mut self) -> &mut Body {
        &mut self.body
    }

    pub fn get_body(self) -> Body {
        self.body
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::core::body::Error;

    #[test]
    fn test_new_message() {
        let data = "test data".to_string();
        let message = Message::new(data.clone());

        assert_eq!(message.type_id(), TypeId::of::<String>());
        assert_eq!(
            message.get_body_ref().get_as_ref::<String>().unwrap(),
            &data
        );
    }

    #[test]
    fn test_type_id() {
        let message_string = Message::new("test".to_string());
        let message_i32 = Message::new(123);

        assert_eq!(message_string.type_id(), TypeId::of::<String>());
        assert_eq!(message_i32.type_id(), TypeId::of::<i32>());
    }

    #[test]
    fn test_get_body_ref() {
        let data = 42;
        let message = Message::new(data);

        let body_ref = message.get_body_ref();
        assert_eq!(body_ref.get_as_ref::<i32>().unwrap(), &data);
    }

    #[test]
    fn test_get_body_mut() {
        let mut message = Message::new(vec![1, 2, 3]);

        let body_mut = message.get_body_mut();
        let vec_mut = body_mut.get_as_mut::<Vec<i32>>().unwrap();
        vec_mut.push(4);

        assert_eq!(
            message.get_body_ref().get_as_ref::<Vec<i32>>().unwrap(),
            &vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn test_get_body() {
        let data = "original".to_string();
        let message = Message::new(data.clone());

        let body = message.get_body();
        let string_body = body.get::<String>().unwrap();
        assert_eq!(*string_body, data);
    }

    #[test]
    fn test_multiple_types() {
        let message_string = Message::new("hello".to_string());
        let message_i32 = Message::new(100);
        let message_bool = Message::new(true);
        let message_struct = Message::new(MyStruct { value: 5 });

        assert_eq!(message_string.type_id(), TypeId::of::<String>());
        assert_eq!(message_i32.type_id(), TypeId::of::<i32>());
        assert_eq!(message_bool.type_id(), TypeId::of::<bool>());
        assert_eq!(message_struct.type_id(), TypeId::of::<MyStruct>());

        assert_eq!(
            message_string
                .get_body_ref()
                .get_as_ref::<String>()
                .unwrap(),
            &"hello".to_string()
        );
        assert_eq!(
            message_i32.get_body_ref().get_as_ref::<i32>().unwrap(),
            &100
        );
        assert_eq!(
            message_bool.get_body_ref().get_as_ref::<bool>().unwrap(),
            &true
        );
        assert_eq!(
            message_struct
                .get_body_ref()
                .get_as_ref::<MyStruct>()
                .unwrap(),
            &MyStruct { value: 5 }
        );
    }

    #[derive(Debug, PartialEq)]
    struct MyStruct {
        value: i32,
    }

    #[test]
    fn test_get_body_ref_type_mismatch() {
        let message = Message::new(42);
        let body_ref = message.get_body_ref();
        let result = body_ref.get_as_ref::<String>();
        assert!(matches!(result, Err(Error::TypeMismatch { .. })));
    }

    #[test]
    fn test_get_body_mut_type_mismatch() {
        let mut message = Message::new(42);
        let body_mut = message.get_body_mut();
        let result = body_mut.get_as_mut::<String>();
        assert!(matches!(result, Err(Error::TypeMismatch { .. })));
    }

    #[test]
    fn test_get_body_type_mismatch() {
        let message = Message::new(42);
        let body = message.get_body();
        let result = body.get::<String>();
        assert!(matches!(result, Err(Error::TypeMismatch { .. })));
    }
}
