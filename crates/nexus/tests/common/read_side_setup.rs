use async_trait::async_trait;
use nexus::{
    Query,
    infra::NexusId,
    query::{ReadModel, ReadModelRepository},
};
use thiserror::Error;

#[derive(Debug, Clone, Query)]
#[query(result = User, error = QueryError)]
pub struct GetUserQuery {
    pub id: NexusId,
}

// Read model
#[derive(Debug)]
pub struct User {
    id: NexusId,
    pub email: String,
}

impl ReadModel for User {
    type Id = NexusId;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum QueryError {
    #[error("User not found")]
    UserNotFound,
}

#[derive(Clone)]
pub struct GetUserRepository {
    fail: bool,
}

#[async_trait]
impl ReadModelRepository for GetUserRepository {
    type Error = QueryError;
    type Model = User;
    async fn get(&self, id: &<Self::Model as ReadModel>::Id) -> Result<Self::Model, Self::Error> {
        if self.fail {
            Err(QueryError::UserNotFound)
        } else {
            Ok(User {
                id: id.clone(),
                email: "joel@tixlys.com".to_string(),
            })
        }
    }
}
// service

pub trait QueryService: Send + Sync {
    fn process(&self) -> String;
}

#[derive(Debug)]
pub struct MockQueryService {
    pub id: String,
}

impl QueryService for MockQueryService {
    fn process(&self) -> String {
        self.id.to_string()
    }
}
