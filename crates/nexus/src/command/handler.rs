use crate::{
    domain::{AggregateType as AT, Command, DomainEvent},
    infra::events::Events,
};
use async_trait::async_trait;
use std::fmt::Debug;

#[derive(Debug)]
pub struct CommandHandlerResponse<E, R>
where
    E: DomainEvent,
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
    type AggregateType: AT;

    async fn handle(
        &self,
        state: &<Self::AggregateType as AT>::State,
        command: C,
        services: &Services,
    ) -> Result<CommandHandlerResponse<<Self::AggregateType as AT>::Event, C::Result>, C::Error>;
}
