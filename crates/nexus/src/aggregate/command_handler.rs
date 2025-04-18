use super::AggregateState;
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

pub type CommandHandlerResult<State, C> = Result<
    CommandHandlerResponse<<State as AggregateState>::Event, <C as Command>::Result>,
    <C as Command>::Error,
>;

pub type CommandHandlerFuture<'a, State, C> =
    Pin<Box<dyn Future<Output = CommandHandlerResult<State, C>> + Send + 'a>>;

pub trait AggregateCommandHandler<C, Services>: Send + Sync
where
    C: Command,
    Services: Send + Sync + ?Sized,
{
    type State: AggregateState;

    fn handle<'a>(
        &'a self,
        state: &'a Self::State,
        command: C,
        services: &'a Services,
    ) -> CommandHandlerFuture<'a, Self::State, C>;
}
