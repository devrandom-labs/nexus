use std::sync::Arc;

use super::raw::RawEventStore;

/// Shared handle to a [`RawEventStore`](super::raw::RawEventStore) backend.
///
/// `Store` wraps the backend in an `Arc`, making it cheap to clone and
/// safe to share across tasks. It carries no codec, upcaster, or
/// aggregate binding — it is just a database handle.
///
/// Use [`repository()`](Store::repository) or
/// [`zero_copy_repository()`](Store::zero_copy_repository) to create
/// typed facades bound to a specific aggregate's codec and upcaster.
///
/// # Example
///
/// ```ignore
/// let store = Store::new(FjallStore::builder("path").open()?);
///
/// let orders = store.repository(OrderCodec, OrderUpcaster);
/// let users  = store.repository(UserCodec, ());
/// ```
#[derive(Debug)]
pub struct Store<S> {
    inner: Arc<S>,
}

impl<S> Store<S> {
    /// Wrap a raw event store backend in a shared handle.
    pub fn new(raw: S) -> Self {
        Self {
            inner: Arc::new(raw),
        }
    }

    /// Access the underlying raw store.
    pub(crate) fn raw(&self) -> &S {
        &self.inner
    }
}

impl<S> Clone for Store<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: RawEventStore> Store<S> {
    /// Create a typed event store (owning codec) for a specific aggregate.
    ///
    /// The returned [`EventStore`](super::event_store::EventStore) implements
    /// [`Repository<A>`](super::repository::Repository) for any aggregate
    /// whose events are compatible with the given codec and upcaster.
    ///
    /// Pass `()` as the upcaster when no schema evolution is needed.
    pub fn repository<C, U>(
        &self,
        codec: C,
        upcaster: U,
    ) -> super::event_store::EventStore<S, C, U> {
        super::event_store::EventStore::new(self.clone(), codec, upcaster)
    }

    /// Create a typed event store (borrowing codec) for a specific aggregate.
    ///
    /// The returned [`ZeroCopyEventStore`](super::zero_copy_event_store::ZeroCopyEventStore)
    /// implements [`Repository<A>`](super::repository::Repository) for any
    /// aggregate whose events are compatible with the given borrowing codec
    /// and upcaster.
    ///
    /// Pass `()` as the upcaster when no schema evolution is needed.
    pub fn zero_copy_repository<C, U>(
        &self,
        codec: C,
        upcaster: U,
    ) -> super::zero_copy_event_store::ZeroCopyEventStore<S, C, U> {
        super::zero_copy_event_store::ZeroCopyEventStore::new(self.clone(), codec, upcaster)
    }
}
