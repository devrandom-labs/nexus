use super::AggregateType;
use crate::{Command, DomainEvent};
use std::{boxed::Box, fmt::Debug, future::Future, pin::Pin};

#[derive(Debug)]
pub struct CommandHandlerResponse<E, R>
where
    E: DomainEvent,
    R: Debug + Send + Sync + 'static,
{
    pub events: Vec<E>,
    pub result: R,
}

/// Each AggregateCommand Handler is tied to a state
pub trait AggregateCommandHandler<C, Services>: Send + Sync
where
    C: Command,
    Services: Send + Sync + ?Sized,
{
    type AT: AggregateType;

    #[allow(clippy::type_complexity)]
    fn handle<'a>(
        &'a self,
        state: &'a <Self::AT as AggregateType>::State,
        command: C,
        services: &'a Services,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        CommandHandlerResponse<<Self::AT as AggregateType>::Event, C::Result>,
                        C::Error,
                    >,
                > + Send
                + 'a,
        >,
    >;
}

#[cfg(test)]
pub mod test {

    use super::{AggregateCommandHandler, CommandHandlerResponse};
    use crate::{
        Command, Message,
        aggregate::{AggregateType, aggregate_root::test::User, test::UserDomainEvents},
    };
    use chrono::Utc;
    use std::{pin::Pin, time::Duration};
    use thiserror::Error as ThisError;
    use tokio::time::sleep;

    #[derive(Debug, ThisError)]
    pub enum UserError {
        #[error("Failed to create user")]
        FailedToCreateUser,
    }

    #[derive(Debug)]
    pub struct CreateUser {
        user_id: String,
        email: String,
    }
    impl Message for CreateUser {}
    impl Command for CreateUser {
        type Result = String;
        type Error = UserError;
    }

    pub struct CreateUserHandler;

    impl AggregateCommandHandler<CreateUser, ()> for CreateUserHandler {
        type AT = User;

        fn handle<'a>(
            &'a self,
            _state: &'a <Self::AT as crate::aggregate::AggregateType>::State,
            command: CreateUser,
            _services: &'a (),
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            super::CommandHandlerResponse<
                                <Self::AT as AggregateType>::Event,
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

    #[tokio::test]
    async fn handler_logic_success() {
        // TODO: set up default state
        // TODO: call the handler
        // TODO: verify that hancler gave the proper event and user_id
    }

    #[tokio::test]
    async fn handler_logic_failure() {
        // TODO: setup default state
        // TODO: create command with error
        // TODO: call the handler
        // TODO: verify you got the correct error
    }
}
