#![allow(dead_code)]
use std::{
    any::{Any, TypeId},
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

//-------------------- ID--------------------//
static ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct Id(usize);
impl Id {
    pub fn new() -> Self {
        let id = ID.fetch_add(1, Ordering::Relaxed);
        Id(id)
    }
}

impl Deref for Id {
    type Target = usize;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

//-------------------- Message--------------------//

#[derive(Clone)]
pub struct Message<T>
where
    T: Any + Send + Sync,
{
    id: Id,
    body: T,
}

impl<T> Message<T>
where
    T: Any + Send + Sync,
{
    pub fn new(body: T) -> Self {
        let id = Id::new();
        Message { id, body }
    }

    pub fn body(&self) -> &T {
        &self.body
    }

    pub fn id(&self) -> usize {
        *self.id
    }

    /// this means that we handle a message based on type
    /// not based on what the instance is of that type.
    ///
    /// all Message<String> would be handled by the same handler.
    pub fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

//-------------------- handlers --------------------//
use tower::{BoxError, Service};

// any service which takes message of any type T as request
// any service which gives message of any type R as response
// is eligible to be a message service.
pub trait MessageService<T, R>:
    Service<Message<T>, Response = Message<R>, Error = BoxError>
where
    T: Send + Sync + 'static,
    R: Send + Sync + 'static,
{
}

#[cfg(test)]
mod test {
    use super::Message;
    use std::thread::spawn;

    #[test]
    fn create_message() {
        let message = Message::new(1);
        assert_eq!(message.body(), &1);
    }

    #[test]
    fn messages_between_threads() {
        let message = Message::new(String::from("Hello"));
        spawn(move || {
            assert_eq!(message.body(), "Hello");
        });
    }

    #[tokio::test]
    async fn messages_between_tasks() {
        let message = Message::new(String::from("Hello"));
        tokio::spawn(async move {
            assert_eq!(message.body(), "Hello");
        });
    }

    #[test]
    fn every_new_message_is_unique() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = Message::new(String::from("Hello"));
        assert_ne!(message_one.id(), message_two.id());
    }

    #[test]
    fn cloned_messages_have_same_id() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = message_one.clone();
        assert_eq!(message_one.id(), message_two.id());
        assert_eq!(message_one.body(), message_two.body());
    }

    #[test]
    fn message_type_id_should_be_different_for_different_types() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = Message::new(0);
        assert_ne!(message_one.type_id(), message_two.type_id());
    }

    #[test]
    fn message_type_id_should_be_same_for_same_type() {
        let message_one = Message::new(String::from("Hello"));
        let message_two = Message::new(String::from("Hello"));
        assert_eq!(message_one.type_id(), message_two.type_id());
    }
}

// //-------------------- handler --------------------//
// use std::clone::Clone;
// use tower::{BoxError, Service};

// /// Message Handlers are specialized tower service.
// /// they take Message as requests and Respond by ()
// /// and emit BusError only.
// ///
// /// extremely specialized.
// pub trait MessageHandler:
//     Service<MessageEnvelope, Response = (), Error = BoxError> + Send + Sync + Clone + 'static
// {
// }

// impl<M, S> MessageHandler<M> for S
// where
//     M: Message,
//     S: Service<MessageEnvelope<M>, Response = (), Error = BoxError> + Send + Sync + Clone + 'static,
// {
// }

// //-------------------- tests --------------------//
// #[cfg(test)]
// mod test {
//     use super::{Message, MessageHandler};
//     use std::any::TypeId;

//     // TODO: Test blanked impl of message for types which have any, sync and send;

//     #[test]
//     fn test_message_impl() {
//         struct TestMessage(String);
//         impl Message for TestMessage {}
//         let s = TestMessage("hello".to_string());
//         assert_eq!(s.type_id(), TypeId::of::<TestMessage>()); // on way to check if Message works.
//     }

//     //-------------------- tower service test --------------------//

//     use std::future::{ready, Ready};
//     use std::task::Poll;
//     use thiserror::Error;
//     use tower::{BoxError, Service};

//     // mock request
//     // a service which is also a good candidate for GreetingService.
//     struct Request(String);
//     #[derive(Debug, Error, PartialEq)]
//     #[error("{0}")]
//     struct GreetingServiceError(String);

//     #[derive(Clone)]
//     struct GreetingService;
//     // need a way to convert all ser
//     impl Service<Request> for GreetingService {
//         type Response = ();
//         type Error = BoxError; // this is too generic,
//         type Future = Ready<Result<Self::Response, Self::Error>>;

//         fn poll_ready(
//             &mut self,
//             _cx: &mut std::task::Context<'_>,
//         ) -> std::task::Poll<Result<(), Self::Error>> {
//             Poll::Ready(Ok(()))
//         }

//         fn call(&mut self, req: Request) -> Self::Future {
//             if req.0.is_empty() {
//                 return ready(Err(BoxError::from(GreetingServiceError(
//                     "req is empty".to_string(),
//                 ))));
//             }
//             ready(Ok(()))
//         }
//     }
//     // mock response

//     #[tokio::test]
//     async fn test_service_handler() {
//         let mut service = GreetingService;
//         let request = Request("request".to_string());
//         let response = service.call(request).await;
//         assert!(response.is_ok());
//     }

//     #[tokio::test]
//     async fn test_service_error() {
//         let mut service = GreetingService;
//         let request = Request("".to_string());
//         let response = service.call(request).await;
//         assert!(response.is_err());
//         // TODO: assert the type of error.
//     }

//     //--------------------message handler tests--------------------//
//     impl Message for Request {}
//     trait AssertMessageHandler<M: Message> {
//         fn assert_in_message_handler(self);
//     }

//     impl<M, S> AssertMessageHandler<M> for S
//     where
//         M: Message,
//         S: MessageHandler<M>,
//     {
//         fn assert_in_message_handler(self) {}
//     }

//     #[tokio::test]
//     async fn specific_services_are_message_handlers() {
//         let service = GreetingService;
//         service.assert_in_message_handler();
//     }

//     use tower::util::BoxCloneSyncService;

//     #[tokio::test]
//     async fn message_handlers_into_box_service() {
//         let greeting_service = GreetingService;
//         let mut service = BoxCloneSyncService::new(greeting_service);
//         let response = service.call(Request("some request".to_string())).await;
//         assert!(response.is_ok());
//     }

//     //TODO: test whether we can erase the type of request and response to Message

//     #[tokio::test]
//     async fn box_service_dynamic_dispatch() {
//         let greeting_service = GreetingService;
//     }
// }
