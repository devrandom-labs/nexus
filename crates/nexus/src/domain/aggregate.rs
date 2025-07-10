use super::{Command, DomainEvent, Id};
use crate::command::{AggregateCommandHandler, CommandHandlerResponse};
use smallvec::{SmallVec, smallvec};
use std::fmt::Debug;

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
}

pub trait AggregateType: Send + Sync + Debug + Copy + Clone + 'static {
    type Id: Id;
    type Event: DomainEvent;
    type State: AggregateState<Event = Self::Event>;
}

pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Id;
    type Event: DomainEvent;
    type State: AggregateState<Event = Self::Event>;

    fn id(&self) -> &Self::Id;

    fn version(&self) -> u64;

    fn state(&self) -> &Self::State;

    fn take_uncommitted_events(&mut self) -> SmallVec<[Self::Event; 1]>;
}

#[derive(Debug)]
pub struct AggregateRoot<AT: AggregateType> {
    id: AT::Id,
    state: AT::State,
    version: u64,
    uncommitted_events: SmallVec<[AT::Event; 1]>,
}

impl<AT> Aggregate for AggregateRoot<AT>
where
    AT: AggregateType,
{
    type Id = AT::Id;
    type Event = AT::Event;
    type State = AT::State;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn state(&self) -> &Self::State {
        &self.state
    }

    fn version(&self) -> u64 {
        self.version
    }

    fn take_uncommitted_events(&mut self) -> SmallVec<[Self::Event; 1]> {
        std::mem::take(&mut self.uncommitted_events)
    }
}

impl<AT> AggregateRoot<AT>
where
    AT: AggregateType,
{
    pub fn new(id: AT::Id) -> Self {
        Self {
            id,
            state: AT::State::default(),
            version: 0,
            uncommitted_events: smallvec![],
        }
    }

    pub fn load_from_history<'h, H>(id: AT::Id, history: H) -> Self
    where
        H: IntoIterator<Item = &'h AT::Event>,
        AT::Id: ToString,
    {
        let mut state = AT::State::default();
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
        Handler: AggregateCommandHandler<C, Services, AggregateType = AT>,
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
