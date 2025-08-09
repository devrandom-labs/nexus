use async_trait::async_trait;
use nexus::{
    command::{AggregateCommandHandler, CommandHandlerResponse},
    domain::{Aggregate, AggregateState, Command},
};
use terrors::OneOf;

pub struct Handler<T>(pub T);

#[async_trait]
pub trait Dispatch<A, C, S>
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
{
    async fn dispatch(
        &self,
        state: &A::State,
        command: C,
        services: &S,
    ) -> Result<
        CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>,
        OneOf<(C::Error, C)>,
    >;
}

#[async_trait]
impl<A, C, S, H> Dispatch<A, C, S> for Handler<H>
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
    H: AggregateCommandHandler<C, S, State = A::State>,
{
    async fn dispatch(
        &self,
        state: &A::State,
        command: C,
        services: &S,
    ) -> Result<
        CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>,
        OneOf<(C::Error, C)>,
    > {
        Ok(self
            .0
            .handle(state, command, services)
            .await
            .map_err(OneOf::new)?)
    }
}

#[async_trait]
impl<A, C, S> Dispatch<A, C, S> for ()
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
{
    async fn dispatch(
        &self,
        _state: &A::State,
        command: C,
        _services: &S,
    ) -> Result<
        CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>,
        OneOf<(C::Error, C)>,
    > {
        Err(OneOf::new(command))
    }
}
