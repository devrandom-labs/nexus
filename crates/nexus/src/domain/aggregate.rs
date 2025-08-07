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

    pub fn load_from_history<'h, H>(id: I, history: H) -> Result<Self>
    where
        H: IntoIterator<Item = &'h VersionedEvent<Box<S::Domain>>>,
    {
        let mut state = S::default();
        let mut last_version = 0u64;

        for versioned_event in history {
            if versioned_event.version != last_version + 1 {
                return Err(Error::SequenceMismatch {
                    stream_id: id.to_string(),
                    expected_version: last_version + 1,
                    actual_version: versioned_event.version,
                });
            }
            state.apply(&versioned_event.event);
            last_version = versioned_event.version;
        }

        Ok(Self {
            id,
            state,
            version: last_version,
            uncommitted_events: smallvec![],
        })
    }

    pub fn apply_events<'a, H>(&mut self, history: H) -> Result<()>
    where
        H: IntoIterator<Item = &'a VersionedEvent<Box<S::Domain>>>,
    {
        for versioned_event in history {
            if versioned_event.version != self.version + 1 {
                return Err(Error::SequenceMismatch {
                    stream_id: self.id().to_string(),
                    expected_version: self.version + 1,
                    actual_version: versioned_event.version,
                });
            }

            self.state.apply(&versioned_event.event);
            self.version = versioned_event.version;
        }
        Ok(())
    }

    pub async fn load_from_stream<'a, H>(id: I, history: H) -> Result<Self>
    where
        H: Stream<Item = Result<&'a VersionedEvent<Box<S::Domain>>>> + Unpin,
    {
        let mut event_stream = history;
        let mut state = S::default();
        let mut last_version = 0u64;
        while let Some(result) = event_stream.next().await {
            let versioned_event = result?;
            if versioned_event.version != last_version + 1 {
                return Err(Error::SequenceMismatch {
                    stream_id: id.to_string(),
                    expected_version: last_version + 1,
                    actual_version: versioned_event.version,
                });
            }
            state.apply(&versioned_event.event);
            last_version = versioned_event.version;
        }
        Ok(Self {
            id,
            state,
            version: last_version,
            uncommitted_events: smallvec![],
        })
    }

    pub async fn apply_event_stream<'a, H>(&mut self, history: H) -> Result<()>
    where
        H: Stream<Item = Result<&'a VersionedEvent<Box<S::Domain>>>> + Unpin,
    {
        let mut event_stream = history;
        while let Some(result) = event_stream.next().await {
            let versioned_event = result?;
            if versioned_event.version != &self.version() + 1 {
                return Err(Error::SequenceMismatch {
                    stream_id: self.id.to_string(),
                    expected_version: self.version() + 1,
                    actual_version: versioned_event.version,
                });
            }
            self.state.apply(&versioned_event.event);
            self.version = versioned_event.version;
        }
        Ok(())
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
        for event in &events {
            self.state.apply(event.as_ref());
        }
        self.uncommitted_events.extend(events);
        Ok(result)
    }
}
