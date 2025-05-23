pub mod utils;
use super::aggregate::{AggregateState, AggregateType};
use super::handler::{AggregateCommandHandler, CommandHandlerResponse};
use crate::{Command, DomainEvent, Message};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{pin::Pin, time::Duration};
use thiserror::Error as ThisError;
use tokio::time::sleep;
// events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UserDomainEvents {
    UserCreated {
        id: String,
        email: String,
        timestamp: DateTime<Utc>,
    },
    UserActivated {
        id: String,
    },
}

impl Message for UserDomainEvents {}

impl DomainEvent for UserDomainEvents {
    type Id = String;
    fn aggregate_id(&self) -> &Self::Id {
        match self {
            Self::UserActivated { id } => id,
            Self::UserCreated { id, .. } => id,
        }
    }
}

// command
#[derive(Debug, ThisError, PartialEq)]
pub enum UserError {
    #[error("Failed to create user")]
    FailedToCreateUser,
}

#[derive(Debug)]
pub struct CreateUser {
    pub user_id: String,
    pub email: String,
}
impl Message for CreateUser {}
impl Command for CreateUser {
    type Result = String;
    type Error = UserError;
}

// state definition
#[derive(Debug, Default, PartialEq)]
pub struct UserState {
    pub email: Option<String>,
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
}

impl UserState {
    pub fn new(email: Option<String>, is_active: bool, created_at: Option<DateTime<Utc>>) -> Self {
        Self {
            email,
            is_active,
            created_at,
        }
    }
}

impl AggregateState for UserState {
    type Event = UserDomainEvents;

    fn apply(&mut self, event: &Self::Event) {
        match event {
            UserDomainEvents::UserCreated {
                email, timestamp, ..
            } => {
                self.email = Some(email.to_string());
                self.created_at = Some(*timestamp);
            }
            UserDomainEvents::UserActivated { .. } => {
                if self.created_at.is_some() {
                    self.is_active = true;
                }
            }
        }
    }
}

// AggregateType
#[derive(Debug, Clone, Copy)]
pub struct User;
impl AggregateType for User {
    type Id = String;
    type Event = UserDomainEvents;
    type State = UserState;
}

// Command handler
pub struct CreateUserHandler;

impl AggregateCommandHandler<CreateUser, ()> for CreateUserHandler {
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        _state: &'a <Self::AggregateType as AggregateType>::State,
        command: CreateUser,
        _services: &'a (),
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        CommandHandlerResponse<
                            <Self::AggregateType as AggregateType>::Event,
                            <CreateUser as Command>::Result,
                        >,
                        <CreateUser as Command>::Error,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let timestamp = Utc::now();
            sleep(Duration::from_secs(2)).await;
            if command.email.contains("error") {
                return Err(UserError::FailedToCreateUser);
            }

            let create_user = UserDomainEvents::UserCreated {
                id: command.user_id.clone(),
                email: command.email,
                timestamp,
            };
            let events = vec![create_user];
            Ok(CommandHandlerResponse {
                events,
                result: command.user_id,
            })
        })
    }
}
