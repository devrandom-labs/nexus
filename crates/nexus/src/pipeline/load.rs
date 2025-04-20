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

#[cfg(test)]
mod test {
    use crate::{
        aggregate::{AggregateRoot, aggregate_root::test::User},
        repository::{EventSourceRepository, RepositoryError},
    };

    // TODO: test success case
    // TODO: aggregate not found case
    // TODO: other repository error

    #[derive(Debug, Clone)]
    struct MockRepository {
        should_return_not_found: bool,
    }

    impl EventSourceRepository for MockRepository {
        type AggregateType = User;

        fn load<'a>(
            &'a self,
            id: &'a <Self::AggregateType as crate::aggregate::AggregateType>::Id,
        ) -> std::pin::Pin<
            Box<
                dyn Future<
                        Output = Result<
                            AggregateRoot<Self::AggregateType>,
                            RepositoryError<
                                <Self::AggregateType as crate::aggregate::AggregateType>::Id,
                            >,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            let id = id.clone();

            if self.should_return_not_found {
                return Box::pin(async move { Err(RepositoryError::AggregateNotFound(id)) });
            }

            Box::pin(async move {
                let aggregate = AggregateRoot::<User>::new(id);
                Ok(aggregate)
            })
        }

        fn save<'a>(
            &'a self,
            _aggregate: AggregateRoot<Self::AggregateType>,
        ) -> std::pin::Pin<
            Box<
                dyn Future<
                        Output = Result<
                            (),
                            RepositoryError<
                                <Self::AggregateType as crate::aggregate::AggregateType>::Id,
                            >,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            todo!()
        }
    }

    #[tokio::test]
    async fn load_service_success() {
        todo!()
    }
}
