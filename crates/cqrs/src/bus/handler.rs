use std::cmp::Eq;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, RwLock};
use tower::{service_fn, util::ServiceFn};

pub struct MessageHandlers<T, F>
where
    T: Hash + Eq,
{
    handlers: Arc<RwLock<HashMap<T, ServiceFn<F>>>>,
}

impl<T, F> MessageHandlers<T, F>
where
    T: Hash + Eq,
{
    pub fn new() -> Self {
        MessageHandlers {
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add(self, message_id: T, handler: F) -> Self {
        let message_handler = service_fn(handler);
        self.handlers
            .write()
            .unwrap()
            .insert(message_id, message_handler);
        self
    }

    pub fn get(&self, message_id: &T) -> &Option<&ServiceFn<F>> {
        &self.handlers.read().unwrap().get(message_id).as_ref()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tower::BoxError;

    #[derive(PartialEq, Eq, Hash)]
    struct Command {
        content: String,
    }

    struct Event {
        content: String,
    }

    #[test]
    fn should_take_function() {
        async fn handle(_command: Command) -> Result<Event, BoxError> {
            let event = Event {
                content: "Reply".to_string(),
            };
            Ok(event)
        }
        let message_handlers = MessageHandlers::new();
        let command = Command {
            content: "Hei Hei".to_string(),
        };
        message_handlers.add(command, handle);
    }
}
