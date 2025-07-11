#![allow(dead_code)]
use async_trait::async_trait;
use nexus::{
    domain::Id,
    infra::NexusId,
    query::{ReadModel, ReadModelRepository},
};
use nexus_macros::Query;
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
    pub fail: bool,
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
    type Result: Id;
    fn process(&self) -> &Self::Result;
}

#[derive(Debug)]
pub struct MockQueryService {
    pub id: NexusId,
}

impl QueryService for MockQueryService {
    type Result = NexusId;
    fn process(&self) -> &Self::Result {
        &self.id
    }
}
