use chrono::{DateTime, Utc};
use nexus::{Aggregate, Command, DomainEvent, domain::AggregateState, infra::NexusId};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

// events
#[derive(DomainEvent, Clone, Serialize, Deserialize, PartialEq, Debug)]
pub enum UserDomainEvents {
    #[domain_event(name = "user_created_v1")]
    UserCreated {
        #[attribute_id]
        id: NexusId,
        email: String,
        timestamp: DateTime<Utc>,
    },
    #[domain_event(name = "user_activated_v1")]
    UserActivated {
        #[attribute_id]
        id: NexusId,
    },
}

// command
#[derive(Debug, ThisError, PartialEq)]
pub enum UserError {
    #[error("Failed to create user")]
    FailedToCreateUser,
    #[error("Failed to activate user")]
    FailedToActivate,
}

#[derive(Debug, Clone, Command)]
#[command(result = String, error = UserError)]
pub struct CreateUser {
    pub user_id: String,
    pub email: String,
}

#[derive(Debug, Command)]
#[command(result = String, error = UserError)]
pub struct ActivateUser {
    pub user_id: String,
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
#[derive(Debug, Clone, Copy, Aggregate)]
#[aggregate(id = NexusId, event = UserDomainEvents, state = UserState)]
pub struct User;

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

pub mod handlers {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use nexus::{
        command::{AggregateCommandHandler, CommandHandlerResponse},
        domain::{AggregateType, Command},
        infra::events::Events,
    };
    use std::{pin::Pin, time::Duration};
    use tokio::time::sleep;

    pub struct CreateUserHandler;
    #[async_trait]
    impl AggregateCommandHandler<CreateUser, ()> for CreateUserHandler {
        type AggregateType = User;

        async fn handle(
            &self,
            _state: &<Self::AggregateType as AggregateType>::State,
            command: CreateUser,
            _services: &(),
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <CreateUser as Command>::Result,
            >,
            <CreateUser as Command>::Error,
        > {
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
            let events = Events::new(create_user);
            Ok(CommandHandlerResponse {
                events,
                result: command.user_id,
            })
        }
    }

    pub struct ActivateUserHandler;

    #[async_trait]
    impl AggregateCommandHandler<ActivateUser, ()> for ActivateUserHandler {
        type AggregateType = User;

        async fn handle(
            &self,
            state: &<Self::AggregateType as AggregateType>::State,
            command: ActivateUser,
            _services: &(),
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <ActivateUser as Command>::Result,
            >,
            <ActivateUser as Command>::Error,
        > {
            sleep(Duration::from_millis(100)).await;

            if state.created_at.is_none() {
                return Err(UserError::FailedToActivate);
            }

            let activate_user = UserDomainEvents::UserActivated {
                id: command.user_id.clone(),
            };

            let events = Events::new(activate_user);
            Ok(CommandHandlerResponse {
                events,
                result: command.user_id,
            })
        }
    }

    pub struct CreateUserHandlerWithService;

    #[async_trait]
    impl AggregateCommandHandler<CreateUser, SomeService> for CreateUserHandlerWithService {
        type AggregateType = User;

        async fn handle(
            &self,
            _state: &<Self::AggregateType as AggregateType>::State,
            command: CreateUser,
            services: &SomeService,
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <CreateUser as Command>::Result,
            >,
            <CreateUser as Command>::Error,
        > {
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
            let events = Events::new(create_user);
            Ok(CommandHandlerResponse {
                events,
                result: services.name.to_owned(),
            })
        }
    }

    pub struct CreateUserWithStateCheck;

    #[async_trait]
    impl AggregateCommandHandler<CreateUser, ()> for CreateUserWithStateCheck {
        type AggregateType = User;

        async fn handle(
            &self,
            state: &<Self::AggregateType as AggregateType>::State,
            command: CreateUser,
            _services: &(),
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <CreateUser as Command>::Result,
            >,
            <CreateUser as Command>::Error,
        > {
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
                let events = Events::new(create_user);
                Ok(CommandHandlerResponse {
                    events,
                    result: command.user_id.to_string(),
                })
            }
        }
    }

    pub struct CreateUserAndActivate;

    #[async_trait]
    impl AggregateCommandHandler<CreateUser, ()> for CreateUserAndActivate {
        type AggregateType = User;

        async fn handle(
            &self,
            state: &<Self::AggregateType as AggregateType>::State,
            command: CreateUser,
            _services: &(),
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <CreateUser as Command>::Result,
            >,
            <CreateUser as Command>::Error,
        > {
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

                let mut events = Events::new(create_user);
                events.add(activate_user);
                Ok(CommandHandlerResponse {
                    events,
                    result: command.user_id.to_string(),
                })
            }
        }
    }

    #[derive(Debug)]
    pub struct ProcessWithDynServiceHandler;

    #[async_trait]
    impl<S: DynTestService + ?Sized> AggregateCommandHandler<CreateUser, S>
        for ProcessWithDynServiceHandler
    {
        type AggregateType = User;

        async fn handle(
            &self,
            _state: &<Self::AggregateType as AggregateType>::State,
            command: CreateUser,
            services: &S,
        ) -> Result<
            CommandHandlerResponse<
                <Self::AggregateType as AggregateType>::Event,
                <CreateUser as Command>::Result,
            >,
            <CreateUser as Command>::Error,
        > {
            let timestamp = Utc::now();
            let processed_data = services.process(&command.email);

            let event = UserDomainEvents::UserCreated {
                id: command.user_id,
                email: command.email,
                timestamp,
            };

            Ok(CommandHandlerResponse {
                events: Events::new(event),
                result: processed_data,
            })
        }
    }
}
