use std::future::Future;

use super::stream::EventStream;
use nexus::{Id, Version};

/// A subscription to events in a single stream.
///
/// Returns an [`EventStream`] that never exhausts — it waits for new
/// events when caught up, rather than returning `None`.
///
/// # Contract
///
/// - `from: None` → start from the beginning of the stream (version 1)
/// - `from: Some(v)` → start from the event *after* version `v`
/// - The returned stream **never returns `None`**. When all existing
///   events have been yielded, it blocks until new events are appended.
/// - Events are yielded with monotonically increasing versions, same
///   as [`EventStream`].
///
/// # Difference from `RawEventStore::read_stream`
///
/// `read_stream` returns a fused stream that terminates when caught up.
/// `subscribe` returns a stream that *waits* instead of terminating.
pub trait Subscription<M: 'static> {
    /// The subscription stream type — an [`EventStream`] that never exhausts.
    ///
    /// The stream's error type must be the same as the subscription's error type
    /// to enable uniform error handling by consumers.
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a
    where
        Self: 'a;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Subscribe to events in a single stream.
    fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: Subscription<M> + Sync, M: 'static> Subscription<M> for &T {
    type Stream<'a>
        = T::Stream<'a>
    where
        Self: 'a;

    type Error = T::Error;

    fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a {
        (**self).subscribe(id, from)
    }
}
