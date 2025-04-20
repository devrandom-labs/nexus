#![allow(dead_code)]
use crate::{
    aggregate::{AggregateRoot, AggregateType},
    repository::{EventSourceRepository, RepositoryError},
};
use std::{
    boxed::Box,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};
use tower::Service;

#[derive(Debug, Clone)]
pub struct LoadService<AT, Repository>
where
    AT: AggregateType,
    Repository: EventSourceRepository<AggregateType = AT>,
{
    _aggregate_type: PhantomData<AT>,
    repository: Repository,
}

impl<AT, Repository> Service<AT::Id> for LoadService<AT, Repository>
where
    AT: AggregateType,
    Repository: EventSourceRepository<AggregateType = AT> + 'static,
{
    type Response = AggregateRoot<AT>;
    type Error = RepositoryError<AT::Id>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&mut self, req: AT::Id) -> Self::Future {
        let repo = self.repository.clone();
        Box::pin(async move { repo.load(&req).await })
    }

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}
