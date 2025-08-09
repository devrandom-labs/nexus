use async_trait::async_trait;
use nexus::{
    command::{AggregateCommandHandler, CommandHandlerResponse},
    domain::{Aggregate, AggregateState, Command},
};

pub enum DispatchOutcome<A, C>
where
    A: Aggregate,
    C: Command,
{
    Handled(
        Result<CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Error>, C::Error>,
    ),
    Unhandled(C),
}

#[async_trait]
pub trait Dispatch<A, C, S>
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
{
    async fn dispatch(&self, state: &A::State, command: C, services: &S) -> DispatchOutcome<A, C>;
}

#[async_trait]
impl<A, C, S> Dispatch<A, C, S> for ()
where
    A: Aggregate,
    C: Command,
    S: Send + Sync + ?Sized,
{
    async fn dispatch(&self, state: &A::State, command: C, services: &S) -> DispatchOutcome<A, C> {
        Err(command)
    }
}

// #[async_trait]
// impl<A, C, S, H, T> Dispatch<A, C, S> for (H, T)
// where
//     A: Aggregate,
//     C: Command,
//     S: Send + Sync + ?Sized,
//     H: AggregateCommandHandler<C, S, State = A::State>,
//     T: Send + Sync,
// {
//     async fn dispatch(
//         &self,
//         state: &A::State,
//         command: C,
//         services: &S,
//     ) -> Result<CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>, C::Error>
//     {
//         self.0.handle(state, command, services).await
//     }
// }

// #[async_trait]
// impl<A, C, S, H, T> CommandHandlerSet<A, C, S> for (H, T)
// where
//     A: Aggregate,
//     C: Command,
//     S: Send + Sync + ?Sized,
//     T: CommandHandlerSet<A, C, S>,
//     H: Send + Sync,
// {
//     async fn handle_command(
//         &self,
//         state: &A::State,
//         command: C,
//         services: &S,
//     ) -> Result<CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>, C::Error>
//     {
//         self.1.handle_command(state, command, services).await
//     }
// }
