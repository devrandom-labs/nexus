use crate::domain::DomainEvent;
use smallvec::{IntoIter as SmallVecIntoIter, SmallVec};
use std::iter::{Chain, Once, once};

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
}

impl<E> From<E> for Events<E>
where
    E: DomainEvent,
{
    fn from(value: E) -> Self {
        Events::new(value)
    }
}

impl<E> IntoIterator for Events<E>
where
    E: DomainEvent,
{
    type Item = E;
    type IntoIter = Chain<Once<E>, SmallVecIntoIter<[E; 1]>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.first).chain(self.more.into_iter())
    }
}

impl<'a, E> IntoIterator for &'a Events<E>
where
    E: DomainEvent,
{
    type Item = &'a E;
    type IntoIter = Chain<Once<&'a E>, core::slice::Iter<'a, E>>;

    fn into_iter(self) -> Self::IntoIter {
        once(&self.first).chain(self.more.iter())
    }
}

// TODO: impl IntoIterator for this collection
// TODO: impl From trait to small_vec

#[macro_export]
macro_rules! events {
    [$head:expr] => {
        {
             Events::new($head)
        }
    };
    [$head:expr, $($tail:expr),+ $(,)?] => {
        {
            let mut events = Events::new($head);
            $(
                events.add($tail);
            )*
                events
        }
    }
}
