use arrayvec::{ArrayVec, IntoIter as ArrayVecIntoIter};

use crate::event::DomainEvent;

use core::iter::{Chain, Once, once};

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

/// A non-empty collection of domain events with compile-time capacity.
///
/// `Events<E, N>` guarantees at least one event (`first`) and can hold
/// up to `N` additional events in a stack-allocated `ArrayVec`. Total
/// capacity is `N + 1`.
///
/// - `N = 0` (default): exactly one event — the common case for most commands.
/// - `N = 2`: up to three events.
///
/// No heap allocation. `no_std` + `no_alloc` compatible.
///
/// Construct via the [`events!`] macro or [`Events::new`].
#[derive(Debug)]
pub struct Events<E: DomainEvent, const N: usize = 0> {
    first: E,
    rest: ArrayVec<E, N>,
}

impl<E: DomainEvent, const N: usize> Events<E, N> {
    #[must_use]
    pub fn new(event: E) -> Self {
        Self {
            first: event,
            rest: ArrayVec::new(),
        }
    }

    /// Add an event to the collection.
    ///
    /// # Panics
    ///
    /// Panics if the collection is at capacity (`N` additional events
    /// already stored). This indicates a programming error — the
    /// `Handle` trait's `N` parameter should match the maximum number
    /// of additional events the handler produces.
    #[allow(
        clippy::expect_used,
        reason = "capacity overflow is a programmer bug, enforced by Handle<C, N>"
    )]
    pub fn add(&mut self, event: E) {
        self.rest.try_push(event).expect(
            "Events capacity exceeded: the Handle trait's N parameter must match the maximum number of additional events",
        );
    }

    /// Returns an iterator over references to the events.
    pub fn iter(&self) -> Chain<Once<&E>, core::slice::Iter<'_, E>> {
        once(&self.first).chain(self.rest.iter())
    }

    #[must_use]
    // Safety: rest.len() <= N and N < usize::MAX (ArrayVec cannot be allocated
    // at usize::MAX capacity), so + 1 cannot overflow.
    pub const fn len(&self) -> usize {
        self.rest.len() + 1
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl<E: DomainEvent + Clone, const N: usize> Clone for Events<E, N> {
    fn clone(&self) -> Self {
        Self {
            first: self.first.clone(),
            rest: self.rest.clone(),
        }
    }
}

impl<E: DomainEvent + PartialEq, const N: usize> PartialEq for Events<E, N> {
    fn eq(&self, other: &Self) -> bool {
        self.first == other.first && self.rest == other.rest
    }
}

impl<E: DomainEvent + Eq, const N: usize> Eq for Events<E, N> {}

impl<E: DomainEvent, const N: usize> From<E> for Events<E, N> {
    fn from(event: E) -> Self {
        Self::new(event)
    }
}

impl<E: DomainEvent, const N: usize> IntoIterator for Events<E, N> {
    type Item = E;
    type IntoIter = Chain<Once<E>, ArrayVecIntoIter<E, N>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.first).chain(self.rest)
    }
}

impl<'a, E: DomainEvent, const N: usize> IntoIterator for &'a Events<E, N> {
    type Item = &'a E;
    type IntoIter = Chain<Once<&'a E>, core::slice::Iter<'a, E>>;

    fn into_iter(self) -> Self::IntoIter {
        once(&self.first).chain(self.rest.iter())
    }
}
