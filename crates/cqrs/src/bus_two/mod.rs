#![allow(dead_code)]
use std::{
    any::{Any, TypeId},
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

// FIXME: Remove ID mechanism, will do everything thorugh type_id
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

//-------------------- Body--------------------//
//
pub struct Body<T: Any + Send + Sync>(Box<T>);

impl<T> Body<T>
where
    T: Any + Send + Sync,
{
    pub fn new(body: T) -> Self {
        Body(Box::new(body))
    }
}

//-------------------- Message--------------------//

/// messages should be able to be sent acorss threads and tasks
/// for messages to be able to send and sync across threads we would need the body to be sent and sync
/// across threads.
///
/// ID inherently is usize, so it can copy/clone and hence its sync + send
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
    pub fn type_id() -> TypeId {
        TypeId::of::<Self>()
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
pub trait MessageService<T, R>:
    Service<Message<T>, Response = Message<R>, Error = BoxError> + Send + Sync + Clone
where
    T: Any + Send + Sync,
    R: Any + Send + Sync,
{
}

// blanket auto impl of Message Service to all services which fall in this category
// any service which has Message<T> for Request/Respone would be treated as MessageService.
impl<S, T, R> MessageService<T, R> for S
where
    S: Service<Message<T>, Response = Message<R>, Error = BoxError> + Send + Sync + Clone,
    T: Any + Send + Sync,
    R: Any + Send + Sync,
{
}

//--------------------Data structure--------------------//
//
//
// should be able to take any type of service
use std::collections::HashMap;

pub struct Route<T, R>
where
    T: Any + Send + Sync,
    R: Any + Send + Sync,
{
    inner: HashMap<TypeId, BoxCloneSyncService<Message<T>, Message<R>, BoxError>>,
}

impl<T, R> Route<T, R>
where
    T: Any + Send + Sync,
    R: Any + Send + Sync,
{
    pub fn new() -> Self {
        Route {
            inner: HashMap::new(),
        }
    }

    pub fn with<M>(mut self, service: M) -> Self
    where
        M: MessageService<T, R> + Send + Sync + 'static,
        M::Future: Send,
    {
        let service = BoxCloneSyncService::new(service);
        self.inner.insert(TypeId::of::<Message<T>>(), service);
        self
    }
}

//-------------------- Test --------------------//
#[cfg(test)]
mod test {
    use super::{Message, MessageService};
    use std::thread::spawn;

    //-------------------- message tests --------------------//
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
        assert_ne!(Message::<String>::type_id(), Message::<u32>::type_id());
    }

    //-------------------- handler tests --------------------//
    //
    //

    use std::any::Any;
    use std::future::{ready, Ready};
    use std::task::{Context, Poll};
    use tower::{BoxError, Service};

    #[derive(Clone)]
    struct CreateUserCommandHandler;
    impl Service<Message<String>> for CreateUserCommandHandler {
        type Response = Message<String>;
        type Error = BoxError;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: Message<String>) -> Self::Future {
            if req.body().is_empty() {
                return ready(Err(BoxError::from("req is empty".to_string())));
            }

            ready(Ok(Message::new("user created".to_string())))
        }
    }

    trait MessageServiceCheck<M, T, R>
    where
        M: MessageService<T, R>,
        T: Any + Sync + Send,
        R: Any + Sync + Send,
    {
        fn assert_message_service(&self);
    }

    impl<M, T, R> MessageServiceCheck<M, T, R> for M
    where
        M: MessageService<T, R>,
        T: Any + Send + Sync,
        R: Any + Send + Sync,
    {
        fn assert_message_service(&self) {}
    }

    #[tokio::test]
    async fn specific_services_should_be_message_services() {
        let create_user_command_handler = CreateUserCommandHandler;
        create_user_command_handler.assert_message_service();
    }

    //-------------------- Route handler test--------------------//
    //
    use super::Route;

    #[tokio::test]
    async fn erase_message_service_concrete_type() {
        let mut _service = CreateUserCommandHandler;
        let mut _route = Route::new().with(_service);
    }
}
