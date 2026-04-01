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
        // Compute version once — avoid 3x recomputation of current_version()
        let next_version = self.current_version().next();
        self.state.apply(&event);
        self.uncommitted_events
            .push(VersionedEvent::new(next_version, event));
        debug_assert_eq!(
            self.current_version(),
            next_version,
            "Invariant violated: apply_event must increment current_version by exactly 1"
        );
    }

    pub fn apply_events(&mut self, events: impl IntoIterator<Item = EventOf<A>>) {
        // Pre-allocate if the iterator has a size hint (Vec, slice, etc.)
        let iter = events.into_iter();
        let (lower, _) = iter.size_hint();
        self.uncommitted_events.reserve(lower);
        for event in iter {
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
            let (version, event) = versioned_event.into_parts();
            if version != expected {
                return Err(KernelError::VersionMismatch {
                    stream_id: aggregate.id.to_string(),
                    expected,
                    actual: version,
                });
            }
            aggregate.state.apply(&event);
            aggregate.version = version;
        }
        debug_assert!(
            aggregate.uncommitted_events.is_empty(),
            "Invariant violated: load_from_events must not produce uncommitted events"
        );
        Ok(aggregate)
    }

    pub fn take_uncommitted_events(&mut self) -> SmallVec<[VersionedEvent<EventOf<A>>; 1]> {
        self.version = self.current_version();
        let events = std::mem::take(&mut self.uncommitted_events);
        debug_assert_eq!(
            self.version, self.current_version(),
            "Invariant violated: after take, version must equal current_version"
        );
        debug_assert!(
            self.uncommitted_events.is_empty(),
            "Invariant violated: after take, uncommitted_events must be empty"
        );
        events
    }
}
