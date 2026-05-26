//! `futures::Stream` bridge for owning-output lending streams.
//!
//! `EventStream<M>` is a GAT lending iterator — each `next()` returns an
//! item that may borrow from the cursor. `futures_core::Stream` has a
//! fixed (non-GAT) `Item` type, so any bridge between the two must
//! materialize the item as an owned value per iteration.
//!
//! This module exposes two pieces:
//!
//! - [`OwnedEventStream`] — a sub-trait of [`EventStream`] that witnesses
//!   "this stream's [`Item<'a>`](EventStream::Item) does not borrow from
//!   the cursor" by declaring a concrete [`OwnedItem`](OwnedEventStream::OwnedItem)
//!   associated type. Implemented for [`Map`] and [`TryMap`] — the
//!   owning-output combinators — and not for the base cursors (whose
//!   `Item<'a>` is a borrowing [`PersistedEnvelope`]).
//! - [`IntoStream`] — the bridge type returned by
//!   [`EventStreamExt::into_stream`](super::cursor::EventStreamExt::into_stream).
//!   Implements [`futures_core::Stream`] by box-pinning the per-iteration
//!   `next()` future. The unfold pattern (move stream into future, return
//!   it back from the future) keeps the bridge non-self-referential, at
//!   the explicit cost of one heap allocation per yielded item.
//!
//! Bridging a raw cursor therefore requires a visible `.map(...)` /
//! `.try_map(...)` step that names the owned target type — making the
//! per-event allocation cost a conscious choice.
//!
//! [`EventStream`]: super::cursor::EventStream
//! [`PersistedEnvelope`]: crate::envelope::PersistedEnvelope
//! [`Map`]: super::combinators::Map
//! [`TryMap`]: super::combinators::TryMap

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::combinators::{Map, MapErr, TryMap};
use super::cursor::EventStream;

// ═══════════════════════════════════════════════════════════════════════════
// OwnedEventStream — sub-trait witness for "Item<'a> is an owned value"
// ═══════════════════════════════════════════════════════════════════════════

/// Sub-trait for streams whose [`Item<'a>`](EventStream::Item) does not
/// borrow from the cursor and can therefore be surfaced through a
/// non-GAT [`futures_core::Stream`].
///
/// Implementations declare a concrete [`OwnedItem`](Self::OwnedItem)
/// associated type and supply [`into_owned`](Self::into_owned), which
/// converts a lent item to its owned representation. For the owning
/// combinators ([`Map`], [`TryMap`]) `into_owned` is the identity — the
/// closure already produced the owned value.
///
/// Hand-implemented (no blanket) by the owning-output combinators only.
/// Base cursors do **not** implement this trait: their `Item<'a>` is a
/// borrowing [`PersistedEnvelope`](crate::envelope::PersistedEnvelope),
/// and forcing a deep-copy here would hide a per-event allocation from
/// callers. To bridge a raw cursor, chain a `.map(...)` or `.try_map(...)`
/// first so the materialization is explicit at the call site.
///
/// # Why a sub-trait
///
/// The natural shape — "`for<'a> Self::Item<'a>: 'static`" plus a way to
/// name the resulting concrete type — is not expressible on stable Rust:
/// the GAT's `where Self: 'a` clause propagates into the HRTB and breaks
/// type equality (`for<'a> Item<'a> = T`). PR1 hit the same wall when
/// trying to bound the decoder facade on
/// `for<'a> Item<'a> = PersistedEnvelope<'a, M>` and resolved it with the
/// `BaseEventStream` sub-trait. This trait is the symmetrical move for
/// the owning case.
pub trait OwnedEventStream<M = ()>: EventStream<M> {
    /// The concrete owned type produced by each yielded item.
    ///
    /// Used as the `Item` type of the [`futures_core::Stream`] returned
    /// by [`EventStreamExt::into_stream`](super::cursor::EventStreamExt::into_stream).
    type OwnedItem: Send + 'static;

    /// Convert a lent item to its owned representation.
    ///
    /// For [`Map`] and [`TryMap`] this is the identity — the closure
    /// already returned the owned value.
    fn into_owned(item: Self::Item<'_>) -> Self::OwnedItem
    where
        Self: Sized;
}

// ─── OwnedEventStream for Map ───────────────────────────────────────────────

impl<S, F, T, M> OwnedEventStream<M> for Map<S, F>
where
    S: EventStream<M> + Send,
    F: for<'a> FnMut(S::Item<'a>) -> T + Send,
    T: Send + 'static,
{
    type OwnedItem = T;

    fn into_owned(item: T) -> T {
        item
    }
}

// ─── OwnedEventStream for TryMap ────────────────────────────────────────────

impl<S, F, T, E, M> OwnedEventStream<M> for TryMap<S, F, E>
where
    S: EventStream<M> + Send,
    F: for<'a> FnMut(S::Item<'a>) -> Result<T, E> + Send,
    T: Send + 'static,
    E: std::error::Error + Send + Sync + 'static + From<S::Error>,
{
    type OwnedItem = T;

    fn into_owned(item: T) -> T {
        item
    }
}

// ─── OwnedEventStream for MapErr ────────────────────────────────────────────

// MapErr passes the inner stream's item through unchanged; the impl follows
// whatever `S` is bound to, mirroring the CARRY-FORWARD note from the
// PR2 deviation entry on `OwnedEventStream` ("if its output is owning, the
// impl follows the inner").
//
// Why `'static` on both `S` and `F`: unlike `Map`/`TryMap`, whose
// `type Item<'a> = T` is a fresh owned type that ignores the GAT lifetime,
// `MapErr<S, F, E2>::Item<'a> = S::Item<'a>` propagates the GAT's
// `where Self: 'a` clause back through `Self`. For the trait method
// `into_owned(item: Self::Item<'_>)` to type-check for any `'_`, every
// type parameter inside `Self` must outlive arbitrary lifetimes —
// `'static` is the standard discharge. This matches actual usage: the
// only consumer of `OwnedEventStream` is
// [`IntoStream`](super::owned::IntoStream), which already requires the
// stream to be `'static + Send + Unpin`.
impl<S, F, E2, M> OwnedEventStream<M> for MapErr<S, F, E2>
where
    S: OwnedEventStream<M> + Send + 'static,
    F: FnMut(S::Error) -> E2 + Send + 'static,
    E2: std::error::Error + Send + Sync + 'static,
{
    type OwnedItem = S::OwnedItem;

    fn into_owned(item: Self::Item<'_>) -> Self::OwnedItem {
        S::into_owned(item)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// IntoStream — futures_core::Stream bridge
// ═══════════════════════════════════════════════════════════════════════════

/// In-flight future yielded by an idle cursor. Owns the cursor outright
/// so it is non-self-referential and `'static`.
type Pending<S, T, E> = Pin<Box<dyn Future<Output = (Result<Option<T>, E>, S)> + Send>>;

/// Bridge from an [`OwnedEventStream`] to a [`futures_core::Stream`].
///
/// Constructed by
/// [`EventStreamExt::into_stream`](super::cursor::EventStreamExt::into_stream).
/// The stream yields `Result<S::OwnedItem, S::Error>` items and terminates
/// when the underlying cursor returns `Ok(None)` or yields an error
/// (errors are surfaced once and then the stream terminates).
///
/// # Cost
///
/// Each `poll_next` that needs to advance the cursor box-pins one async
/// block (the `next()` future plus the owned cursor it captures). This is
/// the per-event allocation the plan calls out as explicit and opt-in.
pub struct IntoStream<S, M>
where
    S: OwnedEventStream<M> + Send + 'static,
{
    state: BridgeState<S, M>,
    _marker: PhantomData<fn() -> M>,
}

enum BridgeState<S, M>
where
    S: OwnedEventStream<M> + Send + 'static,
{
    /// Cursor is idle; ready to start the next iteration.
    Idle(S),
    /// `next()` is in flight; the boxed future owns the cursor.
    Pending(Pending<S, S::OwnedItem, S::Error>),
    /// Stream has terminated (either exhausted or surfaced an error).
    Done,
}

impl<S, M> IntoStream<S, M>
where
    S: OwnedEventStream<M> + Send + 'static,
{
    pub(super) fn new(stream: S) -> Self {
        Self {
            state: BridgeState::Idle(stream),
            _marker: PhantomData,
        }
    }
}

impl<S, M> futures_core::Stream for IntoStream<S, M>
where
    S: OwnedEventStream<M> + Send + Unpin + 'static,
    M: 'static,
{
    type Item = Result<S::OwnedItem, S::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `IntoStream` itself is `Unpin` (it contains no self-referential
        // futures — the boxed future owns its captured `S` outright), so
        // a regular `get_mut` is safe and matches the `unfold` pattern.
        let this = self.get_mut();
        loop {
            match std::mem::replace(&mut this.state, BridgeState::Done) {
                BridgeState::Idle(mut stream) => {
                    // Move the stream into a boxed async block. The future
                    // owns the stream and returns it back when `next()`
                    // resolves, so the bridge can re-enter the Idle state
                    // without any self-referential borrow.
                    //
                    // `S::Item<'_>` borrows from `&mut stream`; we extract
                    // the owned representation via `S::into_owned` inside
                    // the borrow's scope so the mutable borrow ends before
                    // we move `stream` into the returned tuple.
                    let fut: Pending<S, S::OwnedItem, S::Error> = Box::pin(async move {
                        let owned_result = match stream.next().await {
                            Ok(Some(item)) => Ok(Some(S::into_owned(item))),
                            Ok(None) => Ok(None),
                            Err(err) => Err(err),
                        };
                        (owned_result, stream)
                    });
                    this.state = BridgeState::Pending(fut);
                }
                BridgeState::Pending(mut fut) => match fut.as_mut().poll(cx) {
                    Poll::Pending => {
                        this.state = BridgeState::Pending(fut);
                        return Poll::Pending;
                    }
                    Poll::Ready((Ok(Some(item)), stream)) => {
                        this.state = BridgeState::Idle(stream);
                        return Poll::Ready(Some(Ok(item)));
                    }
                    Poll::Ready((Ok(None), _stream)) => {
                        // Stream exhausted naturally; the cursor is dropped.
                        this.state = BridgeState::Done;
                        return Poll::Ready(None);
                    }
                    Poll::Ready((Err(err), _stream)) => {
                        // Surface the error once and terminate.
                        this.state = BridgeState::Done;
                        return Poll::Ready(Some(Err(err)));
                    }
                },
                BridgeState::Done => {
                    this.state = BridgeState::Done;
                    return Poll::Ready(None);
                }
            }
        }
    }
}
