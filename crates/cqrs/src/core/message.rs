use super::body::Body;
use std::any::{Any, TypeId};
use std::collections::HashMap;

// TODO: add hash to messages, since they would use rkyv for this
pub struct Message {
    header: HashMap<String, String>,
    body: Body,
}

impl Message {
    pub fn new<T>(body: T) -> Self
    where
        T: Any + Send + Sync,
    {
        let body = Body::new(body);
        let header = HashMap::new();
        Message { body, header }
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

    use super::Message;

    // get the type from body

    #[test]
    pub fn body_type_id_should_be_different_for_different_types() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = Message::new(12);
        assert_ne!(message_one.type_id(), message_two.type_id());
    }

    #[test]
    pub fn body_type_id_should_be_same_for_same_types() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = Message::new(String::from("Joel"));
        assert_eq!(message_one.type_id(), message_two.type_id());
    }

    #[tokio::test]
    pub async fn body_should_be_transferable_between_threads_and_tasks() {
        let message_one = Message::new(String::from("Hello"));
        let result = tokio::spawn(async move {
            let message_two = Message::new(String::from("Joel"));
            assert_eq!(message_one.type_id(), message_two.type_id());
        })
        .await;
        assert!(result.is_ok());
    }
}
