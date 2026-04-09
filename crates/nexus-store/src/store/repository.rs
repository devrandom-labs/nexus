use nexus::{Aggregate, AggregateRoot, EventOf};
use std::future::Future;

/// Port for loading and saving aggregates via event streams.
///
/// Implementations handle codec encode/decode, streaming rehydration
/// via [`AggregateRoot::replay()`], and version tracking internally.
/// Users interact with aggregates, not envelopes.
///
/// # Stream identity
///
/// The aggregate's `Id` (via `Aggregate::Id`) is used directly as the
/// stream identifier. Adapters are responsible for mapping the `Id` to
/// their internal key format (e.g. string-based key, numeric ID, etc.).
///
/// # Streaming Rehydration
///
/// `load()` streams events from the store one-by-one through `replay()`,
/// enabling zero-allocation rehydration with zero-copy codecs (rkyv,
/// flatbuffers). No intermediate `Vec` allocation is needed.
///
/// # Save contract
///
/// `save()` takes a mutable reference to the aggregate and a slice of
/// decided events (from [`Handle::handle()`](nexus::Handle::handle)).
/// It encodes the events, appends them atomically using
/// `aggregate.version()` as the expected version, and on success
/// calls `advance_version` + `apply_event` per event to sync the
/// in-memory state.
///
/// An empty slice is a silent no-op.
///
/// # Error handling
///
/// Implementations must bridge errors from three sources:
/// - [`RawEventStore`](crate::RawEventStore) errors (I/O, conflicts)
/// - [`Codec`](crate::Codec) errors (serialization failures)
/// - [`KernelError`](nexus::KernelError) (version mismatch during replay)
///
/// [`StoreError`](crate::StoreError) can represent all three via its
/// `Adapter`, `Codec`, and `Kernel` variants. Use `StoreError` as
/// `Self::Error` or define a custom error with `From` impls.
///
/// # Example Implementation Pattern
///
/// ```ignore
/// async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
///     let mut stream = self.store.read_stream(&id, Version::INITIAL).await
///         .map_err(|e| StoreError::Adapter(Box::new(e)))?;
///     let mut root = AggregateRoot::<A>::new(id);
///     while let Some(result) = stream.next().await {
///         let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;
///         let event = self.codec.decode(env.event_type(), env.payload())
///             .map_err(|e| StoreError::Codec(Box::new(e)))?;
///         root.replay(env.version(), &event)?; // KernelError auto-converts via From
///     }
///     Ok(root)
/// }
/// ```
pub trait Repository<A: Aggregate>: Send + Sync {
    /// The error type for repository operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load an aggregate by replaying its event stream.
    ///
    /// Streams events from the store one-by-one through `replay()`,
    /// enabling zero-allocation rehydration with zero-copy codecs.
    /// Returns a fresh aggregate at initial state if the stream is empty.
    fn load(&self, id: A::Id)
    -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    /// Persist decided events and advance the aggregate's in-memory state.
    ///
    /// `events` is the slice of decided events from
    /// [`Handle::handle()`](nexus::Handle::handle). The aggregate's
    /// current [`version()`](AggregateRoot::version) is used as the
    /// expected version for optimistic concurrency.
    ///
    /// An empty `events` slice is a silent no-op (no store interaction).
    ///
    /// On success, calls `advance_version` to the last persisted version
    /// and `apply_event` for each event to keep in-memory state in sync.
    fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
