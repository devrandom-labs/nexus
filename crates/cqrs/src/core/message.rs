use super::body::Body;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::time::Instant;

pub struct Message {
    dedup_id: u32,
    header: HashMap<String, String>,
    body: Body,
    time_stamp: Instant,
}

impl Message {
    pub fn new<T>(dedup_id: u32, body: T) -> Self
    where
        T: Any + Send + Sync,
    {
        let body = Body::new(body);
        let header = HashMap::new();
        Message {
            dedup_id,
            body,
            header,
            time_stamp: Instant::now(),
        }
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
