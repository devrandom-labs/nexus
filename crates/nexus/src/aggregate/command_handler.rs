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
mod test {
    // use super::{AggregateCommandHandler, CommandHandlerFuture};
    // use crate::aggregate::aggregate_root::test::User;
    // pub struct CreateUser {
    //     email: String,
    // }
    // pub struct CreateUserHandler;

    // impl AggregateCommandHandler<CreateUser, ()> for CreateUserHandler {
    //     type AT = User;

    //     fn handle<'a>(
    //         &'a self,
    //         state: &'a <Self::AT as crate::aggregate::AggregateType>::State,
    //         command: CreateUser,
    //         services: &'a (),
    //     ) -> CommandHandlerFuture<'a, Self::AT, CreateUser> {
    //     }
    // }

    #[test]
    fn handler_logic_success() {}

    #[test]
    fn handler_logic_failure() {}
}
