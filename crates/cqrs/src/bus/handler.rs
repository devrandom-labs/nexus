use std::cmp::Eq;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, RwLock};
use tower::{service_fn, util::ServiceFn};

pub struct MessageHandlers<T, F>
where
    T: Hash + Eq,
{
    handlers: Arc<RwLock<HashMap<T, Arc<ServiceFn<F>>>>>,
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
}

#[cfg(test)]
mod test {
    use super::*;
    use std::any::TypeId;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use tower::{service_fn, util::ServiceFn, BoxError, ServiceExt};

    #[derive(PartialEq, Eq, Hash, Clone)]
    struct Command {
        content: String,
    }

    struct Event {
        content: String,
    }

    async fn handle(_command: Command) -> Result<Event, BoxError> {
        let event = Event {
            content: "Reply".to_string(),
        };
        Ok(event)
    }

    #[tokio::test]
    async fn should_take_function() {
        let handlers: Arc<RwLock<HashMap<TypeId, Arc<ServiceFn<_>>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let command = Command {
            content: "Hello".to_string(),
        };
        handlers
            .write()
            .unwrap()
            .insert(TypeId::of::<Command>(), Arc::new(service_fn(handle)));

        let handler = handlers.read().unwrap();
        let handler = handler.get(&TypeId::of::<Command>()).clone();

        if let Some(msg_handler) = handler {
            let response = msg_handler.oneshot(command.clone()).await;
            // Assert the response is Ok and the content is as expected
            match response {
                Ok(event) => {
                    assert_eq!(event.content, "Reply");
                }
                Err(e) => {
                    panic!("Service call failed: {:?}", e);
                }
            }
        } else {
            panic!("Handler not found for command");
        }
    }
}
