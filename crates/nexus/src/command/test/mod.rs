pub mod utils;

use super::{
    NonEmptyEvents,
    aggregate::{AggregateState, AggregateType},
    handler::{AggregateCommandHandler, CommandHandlerResponse},
};
use crate::{Command, DomainEvent, Message};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{pin::Pin, time::Duration};
use thiserror::Error as ThisError;
use tokio::time::sleep;
use tower::Service;

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
    #[error("Failed to activate user")]
    FailedToActivate,
}

#[derive(Debug, Clone)]
pub struct CreateUser {
    pub user_id: String,
    pub email: String,
}
impl Message for CreateUser {}
impl Command for CreateUser {
    type Result = String;
    type Error = UserError;
}

#[derive(Debug)]
pub struct ActivateUser {
    pub user_id: String,
}

impl Message for ActivateUser {}
impl Command for ActivateUser {
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

// service
pub struct SomeService {
    pub name: String,
}

pub trait DynTestService: Send + Sync {
    fn process(&self, input: &str) -> String;
}

#[derive(Debug)]
pub struct MockDynTestService {
    pub prefix: String,
    pub suffix_to_add: Option<String>,
}

impl DynTestService for MockDynTestService {
    fn process(&self, input: &str) -> String {
        let base = format!("{}: {}", self.prefix, input);
        if let Some(suffix) = &self.suffix_to_add {
            format!("{} - {}", base, suffix)
        } else {
            base
        }
    }
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
            sleep(Duration::from_millis(2)).await;
            if command.email.contains("error") {
                return Err(UserError::FailedToCreateUser);
            }

            let create_user = UserDomainEvents::UserCreated {
                id: command.user_id.clone(),
                email: command.email,
                timestamp,
            };
            let events = NonEmptyEvents::new(create_user);
            Ok(CommandHandlerResponse {
                events,
                result: command.user_id,
            })
        })
    }
}

pub struct ActivateUserHandler;

impl AggregateCommandHandler<ActivateUser, ()> for ActivateUserHandler {
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        state: &'a <Self::AggregateType as AggregateType>::State,
        command: ActivateUser,
        _services: &'a (),
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        CommandHandlerResponse<
                            <Self::AggregateType as AggregateType>::Event,
                            <ActivateUser as Command>::Result,
                        >,
                        <ActivateUser as Command>::Error,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            sleep(Duration::from_millis(100)).await;

            if state.created_at.is_none() {
                return Err(UserError::FailedToActivate);
            }

            let activate_user = UserDomainEvents::UserActivated {
                id: command.user_id.clone(),
            };

            let events = NonEmptyEvents::new(activate_user);
            Ok(CommandHandlerResponse {
                events,
                result: command.user_id,
            })
        })
    }
}

pub struct CreateUserHandlerWithService;

impl AggregateCommandHandler<CreateUser, SomeService> for CreateUserHandlerWithService {
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        _state: &'a <Self::AggregateType as AggregateType>::State,
        command: CreateUser,
        services: &'a SomeService,
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
            sleep(Duration::from_millis(2)).await;
            if command.email.contains("error") {
                return Err(UserError::FailedToCreateUser);
            }

            let create_user = UserDomainEvents::UserCreated {
                id: command.user_id.clone(),
                email: command.email,
                timestamp,
            };
            let events = NonEmptyEvents::new(create_user);
            Ok(CommandHandlerResponse {
                events,
                result: services.name.to_owned(),
            })
        })
    }
}

pub struct CreateUserWithStateCheck;

impl AggregateCommandHandler<CreateUser, ()> for CreateUserWithStateCheck {
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        state: &'a <Self::AggregateType as AggregateType>::State,
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
            sleep(Duration::from_millis(2)).await;
            if state.created_at.is_some() {
                Err(UserError::FailedToCreateUser)
            } else {
                let create_user = UserDomainEvents::UserCreated {
                    id: command.user_id.clone(),
                    email: command.email,
                    timestamp,
                };
                let events = NonEmptyEvents::new(create_user);
                Ok(CommandHandlerResponse {
                    events,
                    result: command.user_id.to_string(),
                })
            }
        })
    }
}

pub struct CreateUserAndActivate;

impl AggregateCommandHandler<CreateUser, ()> for CreateUserAndActivate {
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        state: &'a <Self::AggregateType as AggregateType>::State,
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
            sleep(Duration::from_millis(2)).await;
            if state.created_at.is_some() {
                Err(UserError::FailedToCreateUser)
            } else {
                let create_user = UserDomainEvents::UserCreated {
                    id: command.user_id.clone(),
                    email: command.email,
                    timestamp,
                };

                let activate_user = UserDomainEvents::UserActivated {
                    id: command.user_id.clone(),
                };

                let mut events = NonEmptyEvents::new(create_user);
                events.add(activate_user);
                Ok(CommandHandlerResponse {
                    events,
                    result: command.user_id.to_string(),
                })
            }
        })
    }
}

#[derive(Debug)]
pub struct ProcessWithDynServiceHandler;

impl<S: DynTestService + ?Sized> AggregateCommandHandler<CreateUser, S>
    for ProcessWithDynServiceHandler
{
    type AggregateType = User;

    fn handle<'a>(
        &'a self,
        _state: &'a <Self::AggregateType as AggregateType>::State,
        command: CreateUser,
        services: &'a S,
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
            let processed_data = services.process(&command.email);

            let event = UserDomainEvents::UserCreated {
                id: command.user_id,
                email: command.email,
                timestamp,
            };

            Ok(CommandHandlerResponse {
                events: NonEmptyEvents::new(event),
                result: processed_data,
            })
        })
    }
}
