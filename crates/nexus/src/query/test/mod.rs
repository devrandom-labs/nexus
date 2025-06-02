use super::{model::ReadModel, repository::ReadModelRepository};
use std::pin::Pin;
use thiserror::Error;

// Read model
pub struct User {
    id: String,
    pub email: String,
}

impl ReadModel for User {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

#[derive(Debug, Error, Clone)]
pub enum QueryError {
    #[error("User not found")]
    UserNotFound,
}

#[derive(Clone)]
pub struct GetUserRepository;

impl ReadModelRepository for GetUserRepository {
    type Error = QueryError;
    type Model = User;
    fn get<'a>(
        &'a self,
        id: &'a <Self::Model as ReadModel>::Id,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Model, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            if id == "1" {
                Ok(User {
                    id: "1".to_string(),
                    email: "joel@tixlys.com".to_string(),
                })
            } else {
                Err(QueryError::UserNotFound)
            }
        })
    }
}
