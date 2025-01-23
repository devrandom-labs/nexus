use std::any::TypeId;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, RwLock};
use tower::{service_fn, util::ServiceFn};

// https://docs.rs/http/0.2.5/src/http/extensions.rs.html#8-28
// With TypeIds as keys, there's no need to hash them. They are already hashes
// themselves, coming from the compiler. The IdHasher just holds the u64 of
// the TypeId, and then returns it, instead of doing any bit fiddling.
// got the idea from https://github.com/gotham-rs/gotham/blob/main/gotham/src/state/mod.rs
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("TypeId calls write-u64");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

type MessageHandlerMap<F> = HashMap<TypeId, Arc<ServiceFn<F>>, BuildHasherDefault<IdHasher>>;

pub struct MessageHandlers<F> {
    handlers: Arc<RwLock<MessageHandlerMap<F>>>,
}

impl<F> MessageHandlers<F> {
    pub fn new() -> Self {
        MessageHandlers {
            handlers: Arc::new(RwLock::new(HashMap::default())),
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
