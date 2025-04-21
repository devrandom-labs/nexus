use super::invoke_handler::InvokeResponse;
use crate::command::{
    aggregate::AggregateType,
    repository::{EventSourceRepository, RepositoryError},
};
use std::{
    boxed::Box, fmt::Debug, future::Future, marker::PhantomData, pin::Pin, task::Context,
    task::Poll,
};
use tower::Service;

#[derive(Debug, Clone)]
pub struct Persistence<AT, Repository, R>
where
    AT: AggregateType,
    Repository: EventSourceRepository<AggregateType = AT>,
    R: Send + Sync + Debug + 'static,
{
    _aggregate_type: PhantomData<AT>,
    _result: PhantomData<R>,
    repository: Repository,
}

impl<AT, Repository, R> Service<InvokeResponse<AT, R>> for Persistence<AT, Repository, R>
where
    AT: AggregateType,
    Repository: EventSourceRepository<AggregateType = AT> + 'static,
    R: Send + Sync + Debug + 'static,
{
    type Response = R;
    type Error = RepositoryError<AT::Id>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: InvokeResponse<AT, R>) -> Self::Future {
        let repo = self.repository.clone();
        Box::pin(async move {
            let InvokeResponse { aggregate, result } = req;
            repo.save(aggregate).await?;
            Ok(result)
        })
    }
}
