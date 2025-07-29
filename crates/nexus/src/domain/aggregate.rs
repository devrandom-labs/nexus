use super::{Command, DomainEvent, Id};
use crate::command::{AggregateCommandHandler, CommandHandlerResponse};
use smallvec::{SmallVec, smallvec};
use std::fmt::Debug;

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Domain: DomainEvent + ?Sized; // for marker trait for event isolation
    fn apply(&mut self, event: &Self::Domain);
}

pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Id;
    type State: AggregateState;
    fn id(&self) -> &Self::Id;
    fn version(&self) -> u64;
    fn state(&self) -> &Self::State;
    fn take_uncommitted_events(
        &mut self,
    ) -> SmallVec<[Box<<Self::State as AggregateState>::Domain>; 1]>;
}

#[derive(Debug)]
pub struct AggregateRoot<S, I>
where
    S: AggregateState,
    I: Id,
{
    id: I,
    state: S,
    version: u64,
    uncommitted_events: SmallVec<[Box<S::Domain>; 1]>,
}

impl<S, I> Aggregate for AggregateRoot<S, I>
where
    S: AggregateState,
    I: Id,
{
    type Id = I;
    type State = S;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn state(&self) -> &Self::State {
        &self.state
    }

    fn version(&self) -> u64 {
        self.version
    }

    fn take_uncommitted_events(
        &mut self,
    ) -> SmallVec<[Box<<Self::State as AggregateState>::Domain>; 1]> {
        std::mem::take(&mut self.uncommitted_events)
    }
}

impl<S, I> AggregateRoot<S, I>
where
    S: AggregateState,
    I: Id,
{
    pub fn new(id: I) -> Self {
        Self {
            id,
            state: S::default(),
            version: 0,
            uncommitted_events: smallvec![],
        }
    }

    pub fn load_from_history<'h, H>(id: I, history: H) -> Self
    where
        H: IntoIterator<Item = &'h Box<S::Domain>>,
    {
        let mut state = S::default();
        let mut version = 0u64;

        for event in history {
            state.apply(event);
            version += 1;
        }

        Self {
            id,
            state,
            version,
            uncommitted_events: smallvec![],
        }
    }

    pub fn current_version(&self) -> u64 {
        self.version + self.uncommitted_events.len() as u64
    }

    pub async fn execute<C, Handler, Services>(
        &mut self,
        command: C,
        handler: &Handler,
        services: &Services,
    ) -> Result<C::Result, C::Error>
    where
        C: Command,
        Handler: AggregateCommandHandler<C, Services, State = S>,
        Services: Send + Sync + ?Sized,
    {
        let CommandHandlerResponse { events, result } =
            handler.handle(&self.state, command, services).await?;
        for event in &events {
            self.state.apply(event);
        }
        self.uncommitted_events.extend(events);
        Ok(result)
    }
}
