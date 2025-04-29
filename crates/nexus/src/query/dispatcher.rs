#![allow(dead_code)]
use crate::{Query, error::Error};
use std::{
    any::{Any, TypeId},
    boxed::Box,
    collections::HashMap,
    default::Default,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};
use tower::{BoxError, Service, ServiceExt};

pub type BoxMessage = Box<dyn Any + Send>;
pub type ErasedResult = Result<BoxMessage, BoxError>;
pub type ErasedFuture = Pin<Box<dyn Future<Output = ErasedResult> + Send>>;

pub trait ErasedQueryHandlerFn: Send + Sync {
    fn call(&self, query: BoxMessage) -> ErasedFuture;
}

pub struct DispatcherQueryHandler<Q, S>
where
    Q: Query,
    S: Service<Q, Response = Q::Result, Error = Q::Error>,
{
    service: S,
    _query: PhantomData<Q>,
}

impl<Q, S> DispatcherQueryHandler<Q, S>
where
    Q: Query,
    S: Service<Q, Response = Q::Result, Error = Q::Error>,
{
    fn new(service: S) -> Self {
        DispatcherQueryHandler {
            service,
            _query: PhantomData,
        }
    }
}

impl<Q, S> ErasedQueryHandlerFn for DispatcherQueryHandler<Q, S>
where
    Q: Query,
    S: Service<Q, Response = Q::Result, Error = Q::Error> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    fn call(&self, query: BoxMessage) -> ErasedFuture {
        let mut service = self.service.clone();
        Box::pin(async move {
            let query = query.downcast::<Q>().map_err(|_| {
                let type_name_str = std::any::type_name::<Q>();
                BoxError::from(Error::<Q::Error>::InternalError(
                    format!("downcast failed for query: {}", type_name_str).into(),
                ))
            })?;
            let ready_service = service
                .ready()
                .await
                .map_err(|e| BoxError::from(Error::<Q::Error>::InternalError(e.into())))?;
            let result: Q::Result = ready_service
                .call(*query)
                .await
                .map_err(|e| BoxError::from(Error::<Q::Error>::HandlerFailed(e.into())))?;
            Ok(Box::new(result) as BoxMessage)
        })
    }
}

// should query dispatcher builder have Arc<dyn ErasedQueryHandlerFn> ??
pub struct QueryDispatcherBuilder {
    handlers: HashMap<TypeId, Arc<dyn ErasedQueryHandlerFn>>,
}

impl Default for QueryDispatcherBuilder {
    fn default() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }
}

impl QueryDispatcherBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<Q, S>(mut self, service: S) -> Result<Self, Error<Q::Error>>
    where
        Q: Query,
        S: Service<Q, Response = Q::Result, Error = Q::Error> + Clone + Send + Sync + 'static,
        S::Future: Send + Sync,
    {
        let query_type_id = TypeId::of::<Q>();
        let query_name = std::any::type_name::<Q>();
        let dispatcher_handler: Arc<dyn ErasedQueryHandlerFn> =
            Arc::new(DispatcherQueryHandler::<Q, S>::new(service));
        self.handlers
            .insert(query_type_id, dispatcher_handler)
            .ok_or(Error::<Q::Error>::RegistrationFailed(query_name.to_owned()))?;
        Ok(self)
    }
}
