mod common;

use common::read_side_setup::{
    GetUserQuery, GetUserRepository, MockQueryService, QueryError, QueryService, User,
};
use nexus::query::{QueryHandlerFn, ReadModel, ReadModelRepository};
use std::sync::Arc;
use tower::{Service, ServiceExt};

async fn get_user<R>(query: GetUserQuery, repo: R, _service: ()) -> Result<User, QueryError>
where
    // The handler now accepts *any* type `R` that implements the required repository trait.
    R: ReadModelRepository<Model = User, Error = QueryError> + Send,
{
    repo.get(&query.id).await
}

async fn get_user_dyn_service<R>(
    _query: GetUserQuery,
    repo: R,
    service: Arc<dyn QueryService>, // Correctly accepts the shared trait object
) -> Result<User, QueryError>
where
    R: ReadModelRepository<Model = User, Error = QueryError> + Send,
{
    let data = service.process();
    repo.get(&data).await
}

#[tokio::test]
async fn should_be_able_to_execute_query_async_fn() {
    let get_user_repo = GetUserRepository;
    let mut query_handler = QueryHandlerFn::new(get_user, get_user_repo, ());
    let result = query_handler
        .call(GetUserQuery {
            id: "1".to_string(),
        })
        .await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.email, "joel@tixlys.com");
}

#[tokio::test]
async fn should_return_read_model_error_if_according_to_constraints() {
    let get_user_repo = GetUserRepository;

    let mut query_handler = QueryHandlerFn::new(get_user, get_user_repo, ());

    let result = query_handler
        .call(GetUserQuery {
            id: "2".to_string(),
        })
        .await;

    assert!(result.is_err());

    let result = result.unwrap_err();
    assert_eq!(result, QueryError::UserNotFound);
}

#[tokio::test]
async fn should_be_able_to_take_dyn_service() {
    let service = MockQueryService {
        id: "1".to_string(),
    };

    let dyn_services: Arc<dyn QueryService> = Arc::new(service);
    let get_user_repo = GetUserRepository;
    let mut query_handler = QueryHandlerFn::new(get_user_dyn_service, get_user_repo, dyn_services);
    let result = query_handler
        .call(GetUserQuery {
            id: "1".to_string(),
        })
        .await;
    assert!(result.is_ok());
    let user = result.unwrap();
    assert_eq!(user.email, "joel@tixlys.com");
}

#[tokio::test]
async fn should_be_able_call_it_as_tower_service() {
    let handler = QueryHandlerFn::new(get_user, GetUserRepository, ());
    let success_query = GetUserQuery {
        id: "1".to_string(),
    };
    let result = handler.clone().oneshot(success_query).await;

    assert!(result.is_ok());
    let user = result.unwrap();
    assert_eq!(user.email, "joel@tixlys.com");

    let error_query = GetUserQuery {
        id: "2".to_string(),
    };
    let error_result = handler.oneshot(error_query).await;

    assert!(error_result.is_err());
    let error = error_result.unwrap_err();
    assert_eq!(error, QueryError::UserNotFound);
}
