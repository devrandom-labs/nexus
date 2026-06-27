//! Subscription primitive: the user-facing handle that builds the generic
//! catch-up-then-live-tail loop.
//!
//! Users construct [`Subscription::new`] from a [`Store<S>`] and call
//! [`Subscription::subscribe`] / [`Subscription::subscribe_all`] to obtain a
//! `futures::Stream` cursor that **never terminates** — when caught up, it
//! waits for new events rather than yielding `None`. Users never name or touch
//! [`Arc`].
//!
//! # Shape
//!
//! `subscribe`/`subscribe_all` are **synchronous**: wake-registration can fail,
//! so they return `Result<impl Stream, _>` eagerly; read errors stream in-band
//! as `Err` items (see [`live`]). The returned stream is `!Unpin` (it is the
//! `futures::stream::unfold` of the live loop), so consumers MUST `pin!` it
//! before polling — the zero-cost (no-`Box`) tradeoff.
//!
//! # Adapter authoring
//!
//! There is no adapter-facing subscription trait. An adapter need only
//! implement [`RawEventStore`] (the bounded scans) and
//! [`WakeSource`](crate::wake::WakeSource) (the live wake); the generic loop is
//! assembled here from [`StreamCatchup`] / [`AllCatchup`] + [`live`], one
//! monomorphized state machine per call site.

use std::sync::Arc;

use nexus::{Id, Version};

use crate::PersistedEnvelope;
use crate::catchup::{AllCatchup, StreamCatchup};
use crate::store::{GlobalSeq, RawEventStore, Store};
use crate::subscription_cursor::live;
use crate::wake::WakeSource;

/// User-facing subscription handle.
///
/// Holds a shared reference to a [`Store<S>`] backend (one `Arc` clone) and
/// exposes [`subscribe`](Self::subscribe) / [`subscribe_all`](Self::subscribe_all).
/// Cheap to construct; no `Arc` ever appears in user code.
///
/// # Example
///
/// ```ignore
/// use std::pin::pin;
/// use futures::StreamExt;
/// use nexus_store::{Store, Subscription};
///
/// let store = Store::new(FjallStore::builder("path").open()?);
/// let cursor = Subscription::new(&store).subscribe(&account_id, None)?;
/// let mut cursor = pin!(cursor);
/// while let Some(item) = cursor.next().await { /* ... */ }
/// ```
pub struct Subscription<S> {
    store: Arc<S>,
}

impl<S> core::fmt::Debug for Subscription<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Subscription").finish_non_exhaustive()
    }
}

impl<S> Subscription<S> {
    /// Construct from a [`Store<S>`] handle. One `Arc::clone` per call.
    #[must_use]
    pub fn new(store: &Store<S>) -> Self {
        Self {
            store: Arc::clone(store.arc()),
        }
    }
}

impl<S: RawEventStore + WakeSource> Subscription<S> {
    /// Open a per-stream catch-up + live-tail cursor.
    ///
    /// `from: None` starts from version 1; `from: Some(v)` starts from the
    /// event *strictly after* version `v`. The returned stream **never returns
    /// `None`** — it waits for new events when caught up — and is `!Unpin`, so
    /// `pin!` it before polling.
    ///
    /// # Errors
    ///
    /// `<S as WakeSource>::Error` if wake-registration fails. Read errors are
    /// surfaced as `Err` items in the stream (see [`live`]).
    pub fn subscribe<I: Id>(
        &self,
        id: &I,
        from: Option<Version>,
    ) -> Result<
        impl futures_core::Stream<Item = Result<PersistedEnvelope, <S as RawEventStore>::Error>>
        + Send
        + use<S, I>,
        <S as WakeSource>::Error,
    >
    where
        <S as RawEventStore>::Stream: Unpin,
    {
        let catchup = StreamCatchup::new(Arc::clone(&self.store), id.as_ref())?;
        Ok(live(catchup, from))
    }

    /// Open an all-streams (`$all`) catch-up + live-tail cursor in
    /// [`GlobalSeq`] order.
    ///
    /// `from: None` starts from the first event ever appended; `from: Some(g)`
    /// starts from the event *strictly after* `GlobalSeq` `g`. The returned
    /// stream **never returns `None`** and is `!Unpin`, so `pin!` it before
    /// polling.
    ///
    /// # Errors
    ///
    /// `<S as WakeSource>::Error` if wake-registration fails. Read errors are
    /// surfaced as `Err` items in the stream (see [`live`]).
    pub fn subscribe_all(
        &self,
        from: Option<GlobalSeq>,
    ) -> Result<
        impl futures_core::Stream<Item = Result<PersistedEnvelope, <S as RawEventStore>::Error>>
        + Send
        + use<S>,
        <S as WakeSource>::Error,
    >
    where
        <S as RawEventStore>::AllStream: Unpin,
    {
        let catchup = AllCatchup::new(Arc::clone(&self.store))?;
        Ok(live(catchup, from))
    }
}
