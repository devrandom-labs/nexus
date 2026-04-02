use crate::envelope::PendingEnvelope;
use crate::stream::EventStream;
use nexus::Version;

/// What database adapters implement. Bytes in, bytes out.
///
/// Knows nothing about typed events or codecs. The `EventStore` facade
/// calls this trait after encoding events into `PendingEnvelope`.
pub trait RawEventStore<M = ()>: Send + Sync {
    /// The error type for store operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The lending cursor type for reading events.
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a
    where
        Self: 'a;

    /// Append events to a stream with optimistic concurrency.
    ///
    /// `expected_version` is the version the aggregate was at before
    /// new events were applied. The adapter checks this against the
    /// current stream version and rejects if they don't match.
    ///
    /// # Implementor contract
    ///
    /// Envelopes **must** have strictly sequential versions starting from
    /// `expected_version + 1`. Implementations **must** reject batches
    /// where versions are out of order, have gaps, or contain duplicates.
    /// Accepting malformed batches corrupts the event stream.
    fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Open a lending cursor to read events from a stream.
    ///
    /// Events are yielded one at a time via the `EventStream` trait.
    /// Each envelope borrows from the cursor — zero allocation per event.
    fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream<'_>, Self::Error>> + Send;
}
