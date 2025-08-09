use async_trait::async_trait;
use nexus::{
    command::{AggregateCommandHandler, CommandHandlerResponse},
    domain::{Aggregate, AggregateState, Command},
};

#[async_trait]
pub trait CommandHandlerSet<A, C, S>
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
{
    async fn handle_command(
        &self,
        state: &A::State,
        command: C,
        services: &S,
    ) -> Result<CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>, C::Error>;
}
