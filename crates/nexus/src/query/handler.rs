use super::repository::ReadModelRepository;
use crate::Query;
use std::{boxed::Box, marker::PhantomData, pin::Pin, task::Context, task::Poll};
use tower::Service;

#[derive(Clone)]
pub struct QueryHandlerFn<F, Q, R, S>
where
    F: Clone,
    R: Clone,
    S: Clone,
{
    _query: PhantomData<Q>,
    handler: F,
    repo: R,
    services: S,
}

impl<F, Q, R, S> QueryHandlerFn<F, Q, R, S>
where
    F: Clone,
    R: Clone,
    S: Clone,
{
    pub fn new(handler: F, repo: R, services: S) -> Self {
        QueryHandlerFn {
            _query: PhantomData,
            handler,
            repo,
            services,
        }
    }
}

impl<F, Q, R, S> Service<Q> for QueryHandlerFn<F, Q, R, S>
where
    F: Clone,
    Q: Query,
    R: ReadModelRepository + 'static,
    S: Send + Sync + Clone + 'static,
{
    type Response = Q::Result;
    type Error = Q::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Q) -> Self::Future {
        let repo = self.repo.clone();
        let services = self.services.clone();
        let handler = self.handler.clone();
        Box::pin(async move { (handler)(req, repo, services).await })
    }
}
