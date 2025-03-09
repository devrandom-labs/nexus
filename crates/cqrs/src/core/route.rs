use super::message::Message;
use std::any::TypeId;
use std::collections::HashMap;
use tower::{util::BoxCloneSyncService, BoxError};

pub struct Route {
    inner: HashMap<TypeId, BoxCloneSyncService<Message, Message, BoxError>>,
}

impl Route {
    pub fn new() -> Self {
        Route {
            inner: HashMap::new(),
        }
    }
}
