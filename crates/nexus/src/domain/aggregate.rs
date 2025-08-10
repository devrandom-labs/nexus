use super::{Command, DomainEvent, Id};
use crate::{
    command::{AggregateCommandHandler, CommandHandlerResponse},
    error::{Error, Result},
    event::VersionedEvent,
};
use smallvec::{SmallVec, smallvec};
use std::fmt::Debug;
use tokio_stream::{Stream, StreamExt};

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Domain: DomainEvent + ?Sized; // for marker trait for event isolation
    fn apply(&mut self, event: &Self::Domain);
    // to be used as stream_name
    fn name(&self) -> &'static str;
}

pub trait Aggregate: Debug + Send + Sync + 'static {
    type Id: Id;
    type State: AggregateState;
    fn id(&self) -> &Self::Id;
    fn version(&self) -> u64;
    fn state(&self) -> &Self::State;

    #[allow(clippy::type_complexity)]
    fn take_uncommitted_events(
        &mut self,
    ) -> SmallVec<[VersionedEvent<Box<<Self::State as AggregateState>::Domain>>; 1]>;
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
    uncommitted_events: SmallVec<[VersionedEvent<Box<S::Domain>>; 1]>,
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
    ) -> SmallVec<[VersionedEvent<Box<<Self::State as AggregateState>::Domain>>; 1]> {
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

    pub fn load_from_history<H>(id: I, history: H) -> Result<Self>
    where
        H: IntoIterator<Item = VersionedEvent<Box<S::Domain>>>,
    {
        let mut aggregate = Self::new(id);
        rehydrate_from_history(&mut aggregate, history)?;
        Ok(aggregate)
    }

    pub fn apply_events<H>(&mut self, history: H) -> Result<()>
    where
        H: IntoIterator<Item = VersionedEvent<Box<S::Domain>>>,
    {
        rehydrate_from_history(self, history)
    }

    pub async fn load_from_stream<H>(id: I, history: &mut H) -> Result<Self>
    where
        H: Stream<Item = Result<VersionedEvent<Box<S::Domain>>>> + Unpin,
    {
        let mut aggregate = Self::new(id);
        rehydrate_from_stream(&mut aggregate, history).await?;
        Ok(aggregate)
    }

    pub async fn apply_event_stream<H>(&mut self, history: &mut H) -> Result<()>
    where
        H: Stream<Item = Result<VersionedEvent<Box<S::Domain>>>> + Unpin,
    {
        rehydrate_from_stream(self, history).await
    }

    pub fn current_version(&self) -> u64 {
        self.version + self.uncommitted_events.len() as u64
    }

    pub async fn execute<C, Handler, Services>(
        &mut self,
        command: C,
        handler: &Handler,
        services: &Services,
    ) -> std::result::Result<C::Result, C::Error>
    where
        C: Command,
        Handler: AggregateCommandHandler<C, Services, State = S>,
        Services: Send + Sync + ?Sized,
    {
        let CommandHandlerResponse { events, result } =
            handler.handle(&self.state, command, services).await?;
        let mut version_events =
            SmallVec::<[VersionedEvent<Box<S::Domain>>; 1]>::with_capacity(events.len());
        let mut current_version = self.version;
        for event in events {
            self.state.apply(event.as_ref());
            current_version += 1;
            version_events.push(VersionedEvent {
                version: current_version,
                event,
            });
        }
        self.uncommitted_events.extend(version_events);
        Ok(result)
    }
}

async fn rehydrate_from_stream<H, S, I>(
    aggregate: &mut AggregateRoot<S, I>,
    history: &mut H,
) -> Result<()>
where
    H: Stream<Item = Result<VersionedEvent<Box<S::Domain>>>> + Unpin,
    S: AggregateState,
    I: Id,
{
    while let Some(result) = history.next().await {
        let versioned_event = result?;
        if versioned_event.version != aggregate.version + 1 {
            return Err(Error::SequenceMismatch {
                stream_id: aggregate.id.to_string(),
                expected_version: aggregate.version + 1,
                actual_version: versioned_event.version,
            });
        }
        aggregate.state.apply(&versioned_event.event);
        aggregate.version = versioned_event.version;
    }
    Ok(())
}

fn rehydrate_from_history<H, S, I>(aggregate: &mut AggregateRoot<S, I>, history: H) -> Result<()>
where
    H: IntoIterator<Item = VersionedEvent<Box<S::Domain>>>,
    S: AggregateState,
    I: Id,
{
    for versioned_event in history {
        if versioned_event.version != aggregate.version + 1 {
            return Err(Error::SequenceMismatch {
                stream_id: aggregate.id().to_string(),
                expected_version: aggregate.version + 1,
                actual_version: versioned_event.version,
            });
        }
        aggregate.state.apply(&versioned_event.event);
        aggregate.version = versioned_event.version;
    }
    Ok(())
}
