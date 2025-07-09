use crate::domain::DomainEvent;
use smallvec::{SmallVec, smallvec};

#[derive(Debug)]
pub struct Events<E>
where
    E: DomainEvent,
{
    first: E,
    more: SmallVec<[E; 1]>,
}

impl<E> Events<E>
where
    E: DomainEvent,
{
    pub fn new(event: E) -> Self {
        Events {
            first: event,
            more: SmallVec::new(),
        }
    }

    pub fn add(&mut self, event: E) {
        self.more.push(event);
    }

    pub fn into_small_vec(self) -> SmallVec<[E; 1]> {
        let mut events = smallvec![self.first];
        events.extend(self.more);
        events
    }
}

// TODO: impl IntoIterator for this collection
// TODO: impl From trait to small_vec

#[macro_export]
macro_rules! events {
    [$head:expr] => {
        {
            let mut events = Events::new($head);
            events
        }
    };
    [$head:expr, $($tail:expr), +] => {
        {
            let mut events = Events::new($head);
            $(
                events.add($tail);
            )*
                events
        }
    }
}
