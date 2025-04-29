#![allow(dead_code)]
use crate::{Query, error::Error};
use std::{
    any::{Any, TypeId},
    boxed::Box,
    collections::{HashMap, hash_map::Entry},
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};
use thiserror::Error as ThisError;
use tower::{BoxError, Service, ServiceExt};

pub type BoxMessage = Box<dyn Any + Send>;
pub type ErasedResult = Result<BoxMessage, BoxError>;
pub type ErasedFuture = Pin<Box<dyn Future<Output = ErasedResult> + Send + 'static>>;

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
                .map_err(|e| BoxError::from(Error::<Q::Error>::HandlerFailed(e)))?;
            Ok(Box::new(result) as BoxMessage)
        })
    }
}

#[derive(Debug, ThisError)]
#[error("Registration Failed: {0}")]
pub struct RegistrationFailed(String);

#[derive(Default)]
pub struct QueryDispatcherBuilder {
    services: HashMap<TypeId, Arc<dyn ErasedQueryHandlerFn>>,
}

impl QueryDispatcherBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<Q, S>(mut self, service: S) -> Result<Self, RegistrationFailed>
    where
        Q: Query + Any,
        S: Service<Q, Response = Q::Result, Error = Q::Error> + Clone + Send + Sync + 'static,
        S::Future: Send + Sync,
    {
        let query_type_id = TypeId::of::<Q>();
        let dispatcher_services: Arc<dyn ErasedQueryHandlerFn> =
            Arc::new(DispatcherQueryHandler::<Q, S>::new(service));

        match self.services.entry(query_type_id) {
            Entry::Occupied(_) => {
                let query_name = std::any::type_name::<Q>();
                Err(RegistrationFailed(format!(
                    "Service exists for {} query",
                    query_name
                )))
            }
            Entry::Vacant(entry) => {
                entry.insert(dispatcher_services);
                Ok(())
            }
        }?;
        Ok(self)
    }

    pub fn build(self) -> QueryDispatcher {
        QueryDispatcher {
            services: Arc::new(self.services),
        }
    }
}

#[derive(Clone)]
pub struct QueryDispatcher {
    services: Arc<HashMap<TypeId, Arc<dyn ErasedQueryHandlerFn>>>,
}

impl QueryDispatcher {
    pub async fn query<Q>(&self, query: Q) -> Result<Q::Result, Error<Q::Error>>
    where
        Q: Query + Any,
    {
        let query_type_id = TypeId::of::<Q>();
        let type_name_str = std::any::type_name::<Q>();
        let service = self
            .services
            .get(&query_type_id)
            .ok_or(Error::HandlerNotFound(type_name_str.to_owned()))?;
        service
            .call(Box::new(query))
            .await
            .map_err(|e| {
                let c_r = e.downcast::<Error<Q::Error>>().map_err(|_| {
                    Error::<Q::Error>::InternalError(
                        format!("failed to downcast error for query: {}", type_name_str).into(),
                    )
                });
                match c_r {
                    Ok(e) => *e,
                    Err(e) => e,
                }
            })
            .and_then(|r| {
                r.downcast::<Q::Result>()
                    .map_err(|_| {
                        Error::<Q::Error>::InternalError(
                            format!("failed to downcast response for query: {}", type_name_str)
                                .into(),
                        )
                    })
                    .map(|r| *r)
            })
    }

    pub fn builder() -> QueryDispatcherBuilder {
        QueryDispatcherBuilder::default()
    }
}

// TODO: curtail everyone from adding more methdos to query dispatcher.
// TODO: add debug and tracing to dispatchers
// TODO: test every thing!!!
