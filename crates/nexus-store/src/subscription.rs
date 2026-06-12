//! Subscription primitive: concrete user-facing struct + adapter trait.
//!
//! Users construct [`Subscription::new`] from a [`Store<S>`] and call
//! [`Subscription::subscribe`] to obtain a `futures::Stream` cursor that
//! **never terminates** — when caught up, it waits for new events rather
//! than yielding `None`. Users never name or touch [`Arc`].
//!
//! # Adapter authoring
//!
//! Adapters implement [`RawSubscription`] on their bare store type (e.g.
//! `FjallStore`, [`InMemoryStore`](crate::testing::InMemoryStore)), not on
//! `Arc<Store>`. The orphan rule otherwise forbids
//! `impl Subscription for Arc<FjallStore>` in `nexus-fjall` (both
//! `Subscription` and `Arc` foreign to that crate). Delegating through a
//! trait whose `Self` is the bare store type is the standard escape; the
//! user-facing [`Subscription`] struct composes this trait into the
//! external API.
//!
//! # Sealing
//!
//! [`RawSubscription`] is sealed via a `pub` super-trait [`sealed::Sealed`].
//! The `Sealed` trait is reachable from any crate (so adapter crates can
//! implement it), but neither it nor [`RawSubscription`] is re-exported at
//! the crate root. The signal is documentary, not structural: library
//! users grep `Subscription` and find the user-facing struct; adapter
//! authors who need the primitive import the qualified path
//! `nexus_store::subscription::RawSubscription`.

use core::future::Future;
use std::sync::Arc;

use nexus::{Id, Version};

use crate::store::Store;
use crate::stream::EventStream;

/// Sealed-trait scaffold for [`RawSubscription`].
///
/// `Sealed` is `pub` so external adapter crates can implement it on their
/// own store types, but the module containing it is intentionally
/// not re-exported at the crate root — see the module-level docs.
pub mod sealed {
    /// Super-trait that gates [`RawSubscription`](super::RawSubscription)
    /// implementations. Adapter authors `impl sealed::Sealed for
    /// MyStore {}` alongside the `RawSubscription` impl.
    pub trait Sealed {}
}

/// Adapter-facing primitive for subscriptions.
///
/// Implementors expose a `subscribe` associated function (not a method —
/// the first argument is `&Arc<Self>`, not a self-receiver, so the
/// returned cursor can clone the Arc internally and outlive the call).
///
/// The user-facing [`Subscription`] struct composes this trait into the
/// public API. Library users never name `RawSubscription`.
///
/// # Contract
///
/// - `from: None` → start from the first event in the stream (version 1).
/// - `from: Some(v)` → start from the event *after* version `v`.
/// - The returned stream **never returns `None`** — it waits for new events
///   when caught up rather than terminating.
/// - Events are yielded with monotonically increasing versions.
pub trait RawSubscription: sealed::Sealed + Send + Sync + 'static {
    /// The cursor type — a `futures::Stream` of envelopes, `'static`.
    type Stream: EventStream<Error = Self::Error> + 'static;

    /// The error type for subscription operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Open a subscription cursor.
    ///
    /// The first parameter is `&Arc<Self>` (not a self-receiver) so the
    /// adapter can clone the Arc inside and give the returned cursor its
    /// own owned reference to the store.
    fn subscribe(
        arc: &Arc<Self>,
        id: &impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}

/// User-facing subscription handle.
///
/// Holds a shared reference to a [`Store<S>`] backend (one `Arc` clone)
/// and exposes a single async method [`subscribe`](Self::subscribe). Cheap
/// to construct; no `Arc` ever appears in user code.
///
/// # Example
///
/// ```ignore
/// use nexus_store::{Store, Subscription};
///
/// let store = Store::new(FjallStore::builder("path").open()?);
/// let cursor = Subscription::new(&store)
///     .subscribe(&account_id, None)
///     .await?;
/// ```
pub struct Subscription<S> {
    store: Arc<S>,
}

impl<S> core::fmt::Debug for Subscription<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Subscription").finish_non_exhaustive()
    }
}

impl<S: RawSubscription> Subscription<S> {
    /// Construct from a [`Store<S>`] handle. One `Arc::clone` per call.
    #[must_use]
    pub fn new(store: &Store<S>) -> Self {
        Self {
            store: Arc::clone(store.arc()),
        }
    }

    /// Open a subscription cursor.
    ///
    /// `from: None` starts from version 1; `from: Some(v)` starts from
    /// the event *after* version `v`. The returned cursor **never returns
    /// `None`** — it waits for new events when caught up.
    ///
    /// # Errors
    ///
    /// Returns `S::Error` if the adapter cannot open the cursor (e.g. an
    /// I/O failure while seeking to the start position, or an arithmetic
    /// overflow if `from` is `Some(Version::MAX)`).
    pub async fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> Result<S::Stream, S::Error> {
        S::subscribe(&self.store, id, from).await
    }
}
