use crate::domain::{Aggregate, AggregateState, Command, DomainEvent};
use async_trait::async_trait;
use smallvec::SmallVec;
use std::{fmt::Debug, marker::PhantomData};

#[derive(Debug)]
pub struct CommandHandlerResponse<E, R>
where
    E: DomainEvent + ?Sized,
    R: Debug + Send + Sync + 'static,
{
    pub events: SmallVec<[Box<E>; 1]>,
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

pub struct HandlerFn<F, A: Aggregate>(pub F, PhantomData<A>);

pub fn handler<F, A: Aggregate>(handler: F) -> HandlerFn<F, A> {
    HandlerFn(handler, PhantomData)
}

#[async_trait]
impl<F, Fut, A, C, S> AggregateCommandHandler<C, S> for HandlerFn<F, A>
where
    A: Aggregate,
    C: Command,
    F: Fn(&A::State, C, &S) -> Fut + Send + Sync,
    Fut: Future<
            Output = Result<
                CommandHandlerResponse<<A::State as AggregateState>::Domain, C::Result>,
                C::Error,
            >,
        > + Send
        + Sync,
    S: Send + Sync + ?Sized,
{
    type State = A::State;

    async fn handle(
        &self,
        state: &Self::State,
        command: C,
        services: &S,
    ) -> Result<CommandHandlerResponse<<Self::State as AggregateState>::Domain, C::Result>, C::Error>
    {
        (self.0)(state, command, services).await
    }
}
