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

impl<AT, Repository> LoadService<AT, Repository>
where
    AT: AggregateType,
    Repository: EventSourceRepository<AggregateType = AT>,
{
    pub fn new(repository: Repository) -> Self {
        LoadService {
            _aggregate_type: PhantomData,
            repository,
        }
    }
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
    use super::LoadService;
    use crate::{
        aggregate::{Aggregate, AggregateRoot, aggregate_root::test::User},
        repository::{EventSourceRepository, RepositoryError},
    };
    use tower::{Service, ServiceExt};

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
        let mock_repo = MockRepository {
            should_return_not_found: false,
        };
        let mut service = LoadService::new(mock_repo);
        let aggregate_id = "test-user-123".to_string();

        let ready_service = service.ready().await.expect("LoadServce should be ready");
        let result = ready_service.call(aggregate_id.clone()).await;

        assert!(result.is_ok(), "Expected Ok result, got Err: {:?}", result);
        let aggregate_root = result.unwrap();
        assert_eq!(aggregate_root.id(), &aggregate_id);
        assert_eq!(
            aggregate_root.version(),
            0,
            "Newly created aggregate should have version 0"
        );
    }

    #[tokio::test]
    async fn aggregate_not_found() {
        let mock_repo = MockRepository {
            should_return_not_found: true,
        };
        let mut service = LoadService::new(mock_repo);
        let aggregate_id = "test-user-123".to_string();
        let ready_service = service.ready().await.expect("LoadService should be ready");
        let result = ready_service.call(aggregate_id.clone()).await;
        assert!(result.is_err());
        let result = result.unwrap_err();
        assert!(matches!(result, RepositoryError::AggregateNotFound(id) if id == aggregate_id));
    }
}
