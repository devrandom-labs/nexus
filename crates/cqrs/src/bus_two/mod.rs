#![allow(dead_code)]
use std::any::{Any, TypeId};

//-------------------- Body--------------------//
//
pub struct Body {
    inner: Box<dyn Any + Send + Sync>,
}

impl Body {
    pub fn new<T>(data: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Body {
            inner: Box::new(data),
        }
    }

    /// since the inner is trait object of Any
    /// it should have type_id to fetch
    pub fn type_id(&self) -> TypeId {
        (*self.inner).type_id()
    }
}

//-------------------- Message--------------------//

/// messages should be able to be sent acorss threads and tasks
/// for messages to be able to send and sync across threads we would need the body to be sent and sync
/// across threads.
///
/// Messages are not cloneable, they can just be sent and received, why would I need to clone it?
///
/// messages do not require Id,
/// they would require hash?
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
}

//-------------------- handlers --------------------//
use tower::{util::BoxCloneSyncService, BoxError, Service};

// any service which takes message of any type T as request
// any service which gives message of any type R as response
// is eligible to be a message service.
//
// Message service can be executed in any tasks, it has to be send and sync,
//
// if a message service is executed in a task, it would need to last longer than life time of Request
//
pub trait MessageService:
    Service<Message, Response = Message, Error = BoxError> + Send + Sync + Clone
{
}

// blanket auto impl of Message Service to all services which fall in this category
// any service which has Message<T> for Request/Respone would be treated as MessageService.
impl<S> MessageService for S where
    S: Service<Message, Response = Message, Error = BoxError> + Send + Sync + Clone
{
}

//--------------------Data structure--------------------//
//
//
// should be able to take any type of service
use std::collections::HashMap;

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

//-------------------- Test --------------------//
#[cfg(test)]
mod test {

    // type_id should be different for different types in body
    // type_id should be same for same types in body
    // body should be transferable between threads
    // body should be transferable between tasks
    // get the type from body

    #[test]
    pub fn body_type_id_should_be_different_for_different_types() {
        unimplemented!()
    }
    #[test]
    pub fn body_type_id_should_be_same_for_same_types() {
        unimplemented!()
    }

    /// combining both multi threaded and tasks testing in one function
    #[tokio::test]
    pub async fn body_should_be_transferable_between_threads_and_tasks() {}
    #[test]
    pub fn get_reference_of_type_from_body() {}
}
