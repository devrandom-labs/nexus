use crate::event::DomainEvent;
use smallvec::{IntoIter as SmallVecIntoIter, SmallVec};
use std::iter::{Chain, Once, once};

#[macro_export]
macro_rules! events {
    [$head:expr] => {
        $crate::Events::new($head)
    };
    [$head:expr, $($tail:expr),+ $(,)?] => {
        {
            let mut events = $crate::Events::new($head);
            $(
                events.add($tail);
            )*
            events
        }
    };
}

#[derive(Debug)]
pub struct Events<E: DomainEvent> {
    first: E,
    more: SmallVec<[E; 1]>,
}

impl<E: DomainEvent> Events<E> {
    #[must_use]
    pub fn new(event: E) -> Self {
        Self {
            first: event,
            more: SmallVec::new(),
        }
    }

    pub fn add(&mut self, event: E) {
        self.more.push(event);
    }

    /// Returns an iterator over references to the events.
    pub fn iter(&self) -> Chain<Once<&E>, core::slice::Iter<'_, E>> {
        once(&self.first).chain(self.more.iter())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.more.len() + 1
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl<E: DomainEvent> From<E> for Events<E> {
    fn from(event: E) -> Self {
        Self::new(event)
    }
}

impl<E: DomainEvent> IntoIterator for Events<E> {
    type Item = E;
    type IntoIter = Chain<Once<E>, SmallVecIntoIter<[E; 1]>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.first).chain(self.more)
    }
}

impl<'a, E: DomainEvent> IntoIterator for &'a Events<E> {
    type Item = &'a E;
    type IntoIter = Chain<Once<&'a E>, core::slice::Iter<'a, E>>;

    fn into_iter(self) -> Self::IntoIter {
        once(&self.first).chain(self.more.iter())
    }
}
