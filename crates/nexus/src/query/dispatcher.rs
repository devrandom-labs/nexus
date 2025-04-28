#![allow(dead_code)]
use crate::Query;
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
                let err: BoxError =
                    format!("Input downcast failed for query type {}", type_name_str).into();
                err
            })?;

            let ready_service = match service.ready().await {
                Ok(s) => s,
                Err(e) => {
                    return Err(Box::new(e) as BoxError);
                }
            };
            let result: Result<Q::Result, Q::Error> = ready_service.call(*query).await;
            match result {
                Ok(concrete_result) => Ok(Box::new(concrete_result) as BoxMessage),
                Err(concrete_error) => Err(Box::new(concrete_error) as BoxError),
            }
        })
    }
}
