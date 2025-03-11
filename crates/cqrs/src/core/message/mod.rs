use super::body::Body;
use std::any::{Any, TypeId};

pub mod id;

// TODO: add time_stamp with instant for tracing feature
pub struct Message {
    dedup_id: u32,
    body: Body,
}

impl Message {
    pub fn new<T>(dedup_id: u32, body: T) -> Self
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
