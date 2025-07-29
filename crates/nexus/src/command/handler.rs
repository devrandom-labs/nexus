use crate::{
    domain::{AggregateState, Command, DomainEvent},
    infra::events::Events,
};
use async_trait::async_trait;
use std::fmt::Debug;

#[derive(Debug)]
pub struct CommandHandlerResponse<E, R>
where
    E: DomainEvent + ?Sized,
    R: Debug + Send + Sync + 'static,
{
    pub events: Events<E>,

    pub result: R,
}

#[async_trait]
pub trait AggregateCommandHandler<C, Services>: Send + Sync
where
    C: Command,
    Services: Send + Sync + ?Sized,
{
    type State: AggregateState;

    async fn handle(
        &self,
        state: &Self::State,
        command: C,
        services: &Services,
    ) -> Result<CommandHandlerResponse<<Self::State as AggregateState>::Domain, C::Result>, C::Error>;
}
