//! Lending combinators that wrap an [`EventStream`] and yield a different
//! [`Item`](EventStream::Item) projection.
//!
//! Each combinator is itself an `EventStream`, so chains compose: e.g.
//! `stream.try_map(f).try_fold(init, g)`. The base
//! [`Item<'a>`](EventStream::Item) of the cursors is
//! `PersistedEnvelope<'a, M>`; combinators rewrite it according to what
//! their closure produces.
//!
//! # Owning vs. borrowing output
//!
//! [`Map`] and [`TryMap`] restrict their closure to **owning** output —
//! the returned `T` cannot borrow from the lent item. [`TryScan`] is the
//! escape hatch for **borrowing** output: it carries a `State` field that
//! the yielded item borrows from, so each iteration's borrow is tied to
//! `&mut self` of the scan (which holds the state) rather than to the
//! closure's stack.
//!
//! This split avoids the
//! [`lending-iterator`](https://docs.rs/lending-iterator) HKT helper-trait
//! stack while keeping closure ergonomics native.
//!
//! # No `Filter`
//!
//! A natural `Filter` combinator would loop over `self.inner.next()`
//! skipping non-matching items and returning the first match. On stable
//! Rust the borrow checker rejects this body — each iteration's `item`
//! borrow is treated as outliving the loop back-edge, so a
//! `return Ok(Some(item))` in one branch forces `self.inner` to be
//! borrowed across all iterations. Polonius resolves this; stable does
//! not. See the PR1 deviation log for context. Filter returns in a
//! follow-up via Polonius stabilization or a manual state-machine
//! future. For now, the equivalent pattern is to inline the predicate
//! into the closure of [`try_fold`](super::EventStreamExt::try_fold) or
//! [`try_scan`](super::EventStreamExt::try_scan).

use std::marker::PhantomData;

use super::cursor::EventStream;

// ═══════════════════════════════════════════════════════════════════════════
// Map<S, F> — owning per-item transform
// ═══════════════════════════════════════════════════════════════════════════

/// Wraps `S` and applies `F` to each yielded item, producing an owned `T`.
///
/// Built by [`EventStreamExt::map`](super::EventStreamExt::map). The closure
/// bound is `for<'a> FnMut(S::Item<'a>) -> T` — the output `T` cannot borrow
/// from the lent item. For borrowing-output projections (e.g. zero-copy
/// decode into a buffer), use [`TryScan`] instead.
pub struct Map<S, F> {
    pub(crate) inner: S,
    pub(crate) f: F,
}

impl<S, F, T, M> EventStream<M> for Map<S, F>
where
    S: EventStream<M> + Send,
    F: for<'a> FnMut(S::Item<'a>) -> T + Send,
    T: Send,
{
    type Item<'a>
        = T
    where
        Self: 'a;
    type Error = S::Error;

    async fn next(&mut self) -> Result<Option<Self::Item<'_>>, Self::Error> {
        match self.inner.next().await? {
            Some(item) => Ok(Some((self.f)(item))),
            None => Ok(None),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TryMap<S, F, E> — fallible owning per-item transform
// ═══════════════════════════════════════════════════════════════════════════

/// Like [`Map`] but the closure returns `Result<T, E>`.
///
/// On `Err`, the wrapped stream yields the error from `next()`. Stream
/// errors from the underlying cursor are auto-converted via `E: From<S::Error>`.
///
/// Built by [`EventStreamExt::try_map`](super::EventStreamExt::try_map).
pub struct TryMap<S, F, E> {
    pub(crate) inner: S,
    pub(crate) f: F,
    pub(crate) _err: PhantomData<fn() -> E>,
}

impl<S, F, T, E, M> EventStream<M> for TryMap<S, F, E>
where
    S: EventStream<M> + Send,
    F: for<'a> FnMut(S::Item<'a>) -> Result<T, E> + Send,
    T: Send,
    E: std::error::Error + Send + Sync + 'static + From<S::Error>,
{
    type Item<'a>
        = T
    where
        Self: 'a;
    type Error = E;

    async fn next(&mut self) -> Result<Option<Self::Item<'_>>, Self::Error> {
        match self.inner.next().await.map_err(E::from)? {
            Some(item) => (self.f)(item).map(Some),
            None => Ok(None),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// (Filter intentionally not implemented in this iteration — see the
// deviation log: stable Rust's borrow checker can't accept a natural
// loop-based Filter impl over a lending GAT stream, and the Box::pin
// recursion workaround compounds Send/HRTB bounds beyond what's worth
// shipping in PR1. Filter returns in a follow-up via either Polonius
// stabilization or a manual state-machine future.)
// ═══════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════
// TryScan<S, State, F, T, E> — stateful scan; yields borrows from `State`
// ═══════════════════════════════════════════════════════════════════════════

/// Stateful scan over a lending stream — the **borrowing-output** combinator.
///
/// `TryScan` owns a `State` value and threads `&mut State` plus the wrapped
/// stream's item through a closure that returns `Result<&'_ T, E>` (the
/// borrow tied to the same `&mut State`). On each `next()`, the yielded
/// item borrows from `self.state`, so callers can build into a buffer
/// (e.g. a zero-copy decode buffer) and yield references into it without
/// needing HRTB helper-trait stacks on the closure type.
///
/// Built by [`EventStreamExt::try_scan`](super::EventStreamExt::try_scan).
///
/// The closure signature is
/// `for<'s, 'a> FnMut(&'s mut State, S::Item<'a>) -> Result<&'s T, E>`.
/// Both `'s` and `'a` are tied to the same `&mut self` borrow on the scan,
/// so they coexist; the returned `&'s T` is valid until the next call to
/// `next()` invalidates the state borrow.
///
/// `T: ?Sized` so slice-shaped outputs (`&[u8]`, `str`) work.
pub struct TryScan<S, State, F, T: ?Sized, E> {
    pub(crate) inner: S,
    pub(crate) state: State,
    pub(crate) f: F,
    pub(crate) _marker: TryScanMarker<T, E>,
}

/// Variance witness for [`TryScan`]'s unsized payload type and error type.
/// Factored out so the field type doesn't trip `clippy::type_complexity`.
type TryScanMarker<T, E> = PhantomData<fn() -> (Box<T>, E)>;

impl<S, State, F, T, E, M> EventStream<M> for TryScan<S, State, F, T, E>
where
    S: EventStream<M> + Send,
    State: Send,
    T: ?Sized + Send + 'static,
    F: for<'s, 'a> FnMut(&'s mut State, S::Item<'a>) -> Result<&'s T, E> + Send,
    E: std::error::Error + Send + Sync + 'static + From<S::Error>,
{
    type Item<'a>
        = &'a T
    where
        Self: 'a;
    type Error = E;

    async fn next(&mut self) -> Result<Option<Self::Item<'_>>, Self::Error> {
        let Self {
            inner, state, f, ..
        } = self;
        inner
            .next()
            .await
            .map_err(E::from)?
            .map_or_else(|| Ok(None), |item| f(state, item).map(Some))
    }
}
