use super::repository::ReadModelRepository;
use crate::Query;
use std::{
    boxed::Box, clone::Clone, error::Error as StdError, marker::PhantomData, pin::Pin,
    task::Context, task::Poll,
};
use tower::Service;

/// # `QueryHandlerFn<F, Q, R, S, Fut>`
///
/// A struct that wraps a query-handling function `F` and makes it usable as a
/// [`tower::Service<Q>`], where `Q` is the query type.
///
/// This allows query handlers to be easily integrated into `tower`-based service
/// architectures, enabling the use of `tower` middleware (layers) for concerns
/// like logging, metrics, timeouts, retries, etc., around query execution.
///
/// ## Type Parameters:
/// * `F`: The type of the query-handling function. It must be `Fn(Q, R, S) -> Fut`.
///   * It takes the query `Q`, a repository `R`, and other services `S`.
/// * `Q`: The type of the query this handler processes, which must implement [`Query`].
/// * `R`: The type of the [`ReadModelRepository`] used by the handler. It must be `Clone`.
///   The repository's `Model` and `Error` types are constrained by the `Service` impl.
/// * `S`: The type of any additional services or context passed to the handler. It must be `Clone`.
/// * `Fut`: The type of the future returned by the function `F`. It must resolve to
///   `Result<Q::Result, Q::Error>`.
///
/// The struct holds [`PhantomData<Q>`] to acknowledge its association with the query type `Q`
/// at the type level, even if `Q` isn't directly stored as a field.
pub struct QueryHandlerFn<F, Q, R, S, Fut>
where
    F: Fn(Q, R, S) -> Fut,
    Fut: Future<Output = Result<Q::Result, Q::Error>>,
    Q: Query,
    R: Clone,
    S: Clone,
{
    _query: PhantomData<Q>,
    handler: F,
    repo: R,
    services: S,
}

impl<F, Q, R, S, Fut> Clone for QueryHandlerFn<F, Q, R, S, Fut>
where
    F: Fn(Q, R, S) -> Fut + Clone,
    Fut: Future<Output = Result<Q::Result, Q::Error>>,
    Q: Query,
    R: Clone,
    S: Clone,
{
    fn clone(&self) -> Self {
        QueryHandlerFn {
            _query: self._query,
            handler: self.handler.clone(),
            repo: self.repo.clone(),
            services: self.services.clone(),
        }
    }
}

impl<F, Q, R, S, Fut> QueryHandlerFn<F, Q, R, S, Fut>
where
    F: Fn(Q, R, S) -> Fut,
    Fut: Future<Output = Result<Q::Result, Q::Error>>,
    Q: Query,
    R: Clone,
    S: Clone,
{
    /// # Method: `new`
    /// Creates a new `QueryHandlerFn`.
    ///
    /// ### Parameters:
    /// * `handler`: The query-handling function `F`.
    /// * `repo`: An instance of the repository `R` required by the handler.
    /// * `services`: An instance of any additional services/context `S` required.
    pub fn new(handler: F, repo: R, services: S) -> Self {
        QueryHandlerFn {
            _query: PhantomData,
            handler,
            repo,
            services,
        }
    }
}

/// # `tower::Service<Q>` Implementation for `QueryHandlerFn`
///
/// This implementation allows a `QueryHandlerFn` to be treated as a `tower::Service`
/// that processes requests of type `Q` (the query).
///
/// ## Constraints:
/// * `F` (the handler function) must be `Clone + Send + Sync + 'static`.
/// * `Fut` (the future returned by `F`) must be `Send + 'static`.
/// * `Q` (the query) must be `Send + 'static`, and its `Result` and `Error` types
///   must also be `Send + Sync + 'static`. `Q::Error` must also implement `StdError`.
/// * `R` (the repository) must implement [`ReadModelRepository`] and be `'static`. Its `Model`
///   and `Error` types are implicitly constrained by `Q::Result` and `Q::Error` via the
///   handler function `F`, though not directly in this `impl`'s bounds.
/// * `S` (other services) must be `Send + Sync + Clone + 'static`.
///
/// ## Purpose and Usage:
/// By implementing `tower::Service`, `QueryHandlerFn` instances can be:
/// * Composed with `tower::Layer`s to add middleware functionality (logging, metrics, auth, etc.).
/// * Used in `tower` service builders or routers.
/// * Managed by `tower` utilities for concerns like load balancing, rate limiting, etc.
///
/// This promotes a consistent and powerful way to build and manage query processing
/// pipelines within the broader `tower` ecosystem, aligning with `nexus`'s goals for
/// high-performance and composable systems.
impl<F, Q, R, S, Fut> Service<Q> for QueryHandlerFn<F, Q, R, S, Fut>
where
    F: Fn(Q, R, S) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Q::Result, Q::Error>> + Send + 'static,
    Q: Query + Send + 'static,
    Q::Result: Send + Sync + 'static,
    Q::Error: StdError + Send + Sync + 'static,
    R: ReadModelRepository + 'static, // READ MODELS can be diff than what query result
    S: Send + Sync + Clone + 'static,
{
    type Response = Q::Result;
    type Error = Q::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    /// Processes the incoming query `req` by calling the wrapped handler function.
    /// It clones the repository and services for each call, ensuring ownership and
    /// lifetime correctness within the spawned asynchronous task.
    fn call(&mut self, req: Q) -> Self::Future {
        let repo = self.repo.clone();
        let services = self.services.clone();
        let handler = self.handler.clone();
        Box::pin(async move { (handler)(req, repo, services).await })
    }
}

#[cfg(test)]
mod test {
    use super::super::test::{
        GetUserQuery, GetUserRepository, MockQueryService, QueryError, QueryService, User,
    };
    use super::*;
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
        let mut query_handler =
            QueryHandlerFn::new(get_user_dyn_service, get_user_repo, dyn_services);
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
        let mut handler = QueryHandlerFn::new(get_user, GetUserRepository, ());

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
}

// TODO: improve the type of get_user async fn
