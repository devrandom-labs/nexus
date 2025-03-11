use super::body::Body;
use message_id::MessageId;
use std::any::{Any, TypeId};

pub mod error;
pub mod message_id;

// TODO: add time_stamp with instant for tracing feature
pub struct Message {
    dedup_id: MessageId,
    body: Body,
}

impl Message {
    pub fn new<T>(dedup_id: MessageId, body: T) -> Self
    where
        T: Any + Send + Sync,
    {
        let body = Body::new(body);
        Message { dedup_id, body }
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
mod test {}
