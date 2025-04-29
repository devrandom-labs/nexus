#![allow(dead_code)]
use crate::{Query, error::Error};
use std::{any::Any, boxed::Box, marker::PhantomData, pin::Pin};
use tower::{BoxError, Service, ServiceExt};

pub type BoxMessage = Box<dyn Any + Send>;
pub type ErasedResult = Result<BoxMessage, BoxError>;
pub type ErasedFuture = Pin<Box<dyn Future<Output = ErasedResult> + Send>>;

pub trait ErasedQueryHandlerFn: Send + Sync {
    fn call(&self, query: BoxMessage) -> ErasedFuture;
}

pub struct DispatchQueryHandler<Q, S>
where
    Q: Query,
    S: Service<Q, Response = Q::Result, Error = Q::Error>,
{
    service: S,
    _query: PhantomData<Q>,
}

impl<Q, S> ErasedQueryHandlerFn for DispatchQueryHandler<Q, S>
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
