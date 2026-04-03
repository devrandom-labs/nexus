use nexus::{Aggregate, AggregateRoot};
use std::future::Future;

/// Port for loading and saving aggregates via event streams.
///
/// Implementations handle codec encode/decode, streaming rehydration
/// via [`AggregateRoot::replay()`], and version tracking internally.
/// Users interact with aggregates, not envelopes.
///
/// # Streaming Rehydration
///
/// `load()` streams events from the store one-by-one through `replay()`,
/// enabling zero-allocation rehydration with zero-copy codecs (rkyv,
/// flatbuffers). No intermediate `Vec` allocation is needed.
///
/// # Example Implementation Pattern
///
/// ```ignore
/// async fn load(&self, stream_id: &str, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
///     let mut stream = self.store.read_stream(stream_id, Version::INITIAL).await?;
///     let mut root = AggregateRoot::<A>::new(id);
///     while let Some(result) = stream.next().await {
///         let env = result?;
///         let event = self.codec.decode(env.event_type(), env.payload())?;
///         root.replay(env.version(), &event)?;
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
    /// Returns a fresh aggregate at `Version::INITIAL` if the stream is empty.
    fn load(
        &self,
        stream_id: &str,
        id: A::Id,
    ) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;

    /// Persist uncommitted events from an aggregate.
    ///
    /// Drains uncommitted events via `take_uncommitted_events()`,
    /// encodes them, and appends to the store with optimistic concurrency.
    /// The aggregate's persisted version advances on success.
    fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
