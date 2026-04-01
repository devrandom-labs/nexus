use super::error::KernelError;
use super::event::DomainEvent;
use super::id::Id;
use super::version::{Version, VersionedEvent};
use smallvec::{SmallVec, smallvec};
use std::error::Error;
use std::fmt::Debug;

pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
    fn name(&self) -> &'static str;
}

pub trait Aggregate {
    type State: AggregateState;
    type Error: Error + Send + Sync + Debug + 'static;
    type Id: Id;
}

pub type EventOf<A> = <<A as Aggregate>::State as AggregateState>::Event;

#[derive(Debug)]
pub struct AggregateRoot<A: Aggregate> {
    id: A::Id,
    state: A::State,
    version: Version,
    uncommitted_events: SmallVec<[VersionedEvent<EventOf<A>>; 1]>,
}

impl<A: Aggregate> AggregateRoot<A> {
    pub fn new(id: A::Id) -> Self {
        Self {
            id,
            state: A::State::default(),
            version: Version::INITIAL,
            uncommitted_events: smallvec![],
        }
    }

    pub fn id(&self) -> &A::Id {
        &self.id
    }

    pub fn state(&self) -> &A::State {
        &self.state
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn current_version(&self) -> Version {
        let uncommitted_count = self.uncommitted_events.len() as u64;
        Version::new(self.version.as_u64() + uncommitted_count)
    }

    pub fn apply_event(&mut self, event: EventOf<A>) {
        self.state.apply(&event);
        let version = self.current_version().next();
        self.uncommitted_events
            .push(VersionedEvent::new(version, event));
    }

    pub fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<A>>) {
        for event in events {
            self.apply_event(event);
        }
    }

    pub fn load_from_events(
        id: A::Id,
        events: impl IntoIterator<Item = VersionedEvent<EventOf<A>>>,
    ) -> Result<Self, KernelError> {
        let mut aggregate = Self::new(id);
        for versioned_event in events {
            let expected = aggregate.version.next();
            if versioned_event.version() != expected {
                return Err(KernelError::VersionMismatch {
                    stream_id: aggregate.id.to_string(),
                    expected,
                    actual: versioned_event.version(),
                });
            }
            let (version, event) = versioned_event.into_parts();
            aggregate.state.apply(&event);
            aggregate.version = version;
        }
        Ok(aggregate)
    }

    pub fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<A>>; 1]> {
        self.version = self.current_version();
        std::mem::take(&mut self.uncommitted_events)
    }
}
