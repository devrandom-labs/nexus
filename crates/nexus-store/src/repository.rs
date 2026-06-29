// `try_fold` closure bodies clone Arc-wrapped codec/upcast captures from outer
// scope and re-bind them by the same name — clippy flags as `shadow_reuse`,
// but the rebinding is idiomatic for per-iteration Arc clones in async
// combinator chains and renaming everywhere would just add noise.
#![allow(
    clippy::shadow_reuse,
    reason = "per-iteration Arc clones in try_fold closures intentionally re-bind"
)]

use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::sync::Arc;

use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Events, Version};

use futures::TryStreamExt;

use crate::codec::{Decode, Encode};
use crate::envelope::{PersistedEnvelope, pending_envelope};
use crate::error::{AppendError, LoadWithError, StoreError};
use crate::store::{RawEventStore, Store};
use crate::upcasting::EventMorsel;
use crate::value::SchemaVersion;

// ═══════════════════════════════════════════════════════════════════════════
// Repository<A> — high-level aggregate facade (load + save)
// ═══════════════════════════════════════════════════════════════════════════

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
/// `save()` takes a mutable reference to the aggregate and the
/// non-empty [`Events<E, N>`](nexus::Events) decided by
/// [`Handle::handle()`](nexus::Handle::handle). It encodes the events,
/// appends them atomically using `aggregate.version()` as the expected
/// version, and on success calls `commit_persisted` to advance the version
/// and fold the events into in-memory state atomically.
///
/// Taking `&Events<E, N>` (not `&[EventOf<A>]`) carries the kernel's
/// `>= 1` guarantee through to persistence: an empty save is
/// unrepresentable, so there is no runtime no-op case to guard.
///
/// # Schema evolution
///
/// The trait surface does not carry an upcaster — `load()` reads events
/// at their stored schema version and decodes them directly, while `save()`
/// stamps `Version::INITIAL` as the schema version on each new event. For
/// schema evolution, drop to the concrete facade and call its inherent
/// [`load_with`](EventStore::load_with) /
/// [`save_with`](EventStore::save_with) methods (or compose the
/// substrate via [`Store::raw`](crate::Store::raw)).
///
/// # Error handling
///
/// Implementations must bridge errors from four sources:
/// - [`RawEventStore`](crate::RawEventStore) errors (I/O, conflicts)
/// - [`Encode`](crate::Encode) errors (serialization failures on write)
/// - [`Decode`](crate::Decode) errors (deserialization failures on read)
/// - [`KernelError`](nexus::KernelError) (version mismatch during replay)
///
/// [`StoreError`](crate::StoreError) can represent all four via its
/// `Adapter`, `Encode`, `Decode`, and `Kernel` variants. Use `StoreError`
/// as `Self::Error` or define a custom error with `From` impls.
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
    /// `events` is the non-empty [`Events<E, N>`](nexus::Events) decided by
    /// [`Handle::handle()`](nexus::Handle::handle). The aggregate's
    /// current [`version()`](AggregateRoot::version) is used as the
    /// expected version for optimistic concurrency.
    ///
    /// The `&Events<EventOf<A>, N>` parameter guarantees at least one
    /// event at compile time — there is no empty-input case.
    ///
    /// On success, calls `commit_persisted` with the last persisted version to
    /// advance the version and fold the events into in-memory state atomically.
    fn save<const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// ReplayFrom<A> — pub(crate) trait shared with the Snapshotting decorator
// ═══════════════════════════════════════════════════════════════════════════

/// Internal trait for replaying events from a given starting point.
///
/// Both [`EventStore`] and [`ZeroCopyEventStore`] implement this so the
/// [`Snapshotting`](crate::snapshot::Snapshotting) decorator can share
/// replay logic. Not public API.
pub(crate) trait ReplayFrom<A: Aggregate>: Send + Sync {
    /// The error type for replay operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Replay events starting from `from` version (inclusive) into `root`.
    ///
    /// Returns the updated aggregate with all events applied.
    fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a `Version` (`NonZeroU64`) to a `NonZeroU32` for the envelope's
/// `schema_version` field. Returns `None` if the version exceeds `u32::MAX`.
pub(super) fn version_to_nz32(version: Version) -> Option<NonZeroU32> {
    let raw = version.as_u64();
    let narrow = u32::try_from(raw).ok()?;
    // SAFETY: Version wraps NonZeroU64, so raw >= 1, so narrow >= 1.
    NonZeroU32::new(narrow)
}

/// The first [`Version`] an append will assign, given the stream's current
/// version (`None` = empty stream). Returns `None` on overflow past `u64::MAX`.
///
/// Single source of truth for the "next version to write" computation shared by
/// the aggregate save paths and the saga repository's intent-version pinning —
/// keeps the arithmetic checked in exactly one place (CLAUDE.md rule 2).
pub(crate) const fn first_persisted_version(current: Option<Version>) -> Option<Version> {
    match current {
        None => Some(Version::INITIAL),
        Some(v) => v.next(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// EventStore — owning codec (serde, bincode, postcard, etc.)
// ═══════════════════════════════════════════════════════════════════════════

/// Event store using owning [`Encode`] + [`Decode`] — one allocation per decoded event.
///
/// For serde-based formats where deserialization produces an owned value.
///
/// # Construction
///
/// Created via [`Store::repository::<A>()`](crate::store::Store::repository),
/// which names the aggregate `A` once:
///
/// ```ignore
/// let store = Store::new(backend);
/// let orders = store.repository::<Order>().codec(OrderCodec).build();
/// let order = orders.load(id).await?;        // AggregateRoot<Order> — inferred
/// orders.save(&mut order, &events).await?;   // inferred
/// ```
///
/// # Aggregate binding
///
/// The aggregate `A` is a phantom type parameter (carried as
/// `PhantomData<fn() -> A>`, so the facade is `Send + Sync + 'static`
/// regardless of `A` and stays covariant in it). It exists solely so the
/// facade implements [`Repository<A>`] for **exactly one** `A`: with `A`
/// fixed on the type, `load(id)` / `save(..)` infer the aggregate from the
/// receiver, with no per-call annotation (the blanket-over-`A` impl that
/// previously defeated inference is gone). `A` is named once, at
/// `repository::<A>()`. The substrate [`Store<S>`] remains multi-aggregate;
/// mint one cheap per-aggregate facade per aggregate type.
///
/// # Schema evolution
///
/// The plain [`load`](Repository::load) / [`save`](Repository::save) path
/// performs no upcasting. For schema evolution, call
/// [`load_with`](Self::load_with) with the macro-generated function
/// (e.g. `OrderTransforms::upcast`) on the read path, and
/// [`save_with`](Self::save_with) with `OrderTransforms::current_version`
/// on the write path:
///
/// ```ignore
/// // Read path:
/// let root = es.load_with(id, OrderTransforms::upcast).await?;
///
/// // Write path:
/// es.save_with(&mut root, &events, OrderTransforms::current_version).await?;
/// ```
///
/// # Internal ownership
///
/// Owns the codec as `Arc<C>` so async load paths can clone the handle
/// into combinator closures and capture it by value. Per Rust 2024's
/// stricter capture rules (RFC 3498, rustc issue 133529), a closure that
/// borrows from `&self` and is then handed to a `try_fold`-style
/// combinator whose returned future is `+ Send` cannot satisfy the
/// bound — the future-Send check effectively requires the borrow to be
/// `'static`. Owning the component via `Arc` and cloning per call
/// sidesteps the borrow entirely. Cost: one heap allocation at facade
/// construction, one pointer bump per `load`.
pub struct EventStore<S, C, A> {
    store: Store<S>,
    codec: Arc<C>,
    _aggregate: PhantomData<fn() -> A>,
}

impl<S, C, A> EventStore<S, C, A> {
    /// Create an event store bound to a shared store and codec for aggregate `A`.
    pub(crate) fn new(store: Store<S>, codec: C) -> Self {
        Self {
            store,
            codec: Arc::new(codec),
            _aggregate: PhantomData,
        }
    }
}

impl<S, C, A> ReplayFrom<A> for EventStore<S, C, A>
where
    A: Aggregate,
    S: RawEventStore + 'static,
    for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = EventOf<A>> + 'static,
    EventOf<A>: DomainEvent,
    S::Stream: Send,
{
    type Error =
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>;

    async fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, Self::Error> {
        // Clone everything into function-local owned values. The
        // combinator closure captures the locals (Arc clones), with no
        // borrow of `&self`. See the doc comment on `EventStore` for the
        // full Rust 2024 capture-rules rationale.
        let store = self.store.clone();
        let codec = Arc::<C>::clone(&self.codec);

        let raw_stream = store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(StoreError::Adapter)?;

        raw_stream
            .map_err(StoreError::Adapter)
            .try_fold(root, move |mut r, env| {
                let codec = Arc::<C>::clone(&codec);
                async move {
                    let version = env.version();
                    let event: EventOf<A> = <C as Decode<EventOf<A>>>::decode(&codec, &env)
                        .map_err(StoreError::Decode)?;
                    r.replay(version, &event)?;
                    Ok(r)
                }
            })
            .await
    }
}

impl<S, C, A> Repository<A> for EventStore<S, C, A>
where
    A: Aggregate,
    S: RawEventStore + 'static,
    for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = EventOf<A>> + 'static,
    EventOf<A>: DomainEvent,
    S::Stream: Send,
{
    type Error =
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
        let root = AggregateRoot::<A>::new(id);
        self.replay_from(root, Version::INITIAL).await
    }

    async fn save<const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
    ) -> Result<(), Self::Error> {
        // The no-upcaster save stamps Version::INITIAL as the schema
        // version on every event — the schema-version-lookup function
        // is only needed when an upcaster is in play. See `save_with`.
        save_owning::<A, S, C, _, N>(&self.store, &self.codec, aggregate, events, |_| None).await
    }
}

impl<S, C, A> EventStore<S, C, A> {
    /// Load an aggregate, running `upcast` over each persisted event
    /// before decoding it.
    ///
    /// `upcast` is the schema-evolution function — typically the
    /// associated function the `#[nexus::transforms]` macro emits
    /// (e.g. `OrderTransforms::upcast`). Pass it directly as a function
    /// pointer; the `'static` bound on `F` and the `+ Send + Sync` bounds
    /// are required by the `try_fold` combinator chain (see the doc
    /// comment on [`EventStore`] for the full Rust 2024 capture-rules
    /// rationale).
    ///
    /// # Errors
    ///
    /// Returns [`LoadWithError::Store`] for any non-upcast error
    /// (adapter, codec, kernel) and [`LoadWithError::Upcast`] for any
    /// error returned by the `upcast` function.
    pub async fn load_with<F, E>(
        &self,
        id: A::Id,
        upcast: F,
    ) -> Result<
        AggregateRoot<A>,
        LoadWithError<
            S::Error,
            <C as Encode<EventOf<A>>>::Error,
            <C as Decode<EventOf<A>>>::Error,
            E,
        >,
    >
    where
        A: Aggregate,
        S: RawEventStore + 'static,
        for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = EventOf<A>> + 'static,
        F: for<'a> Fn(EventMorsel<'a>) -> Result<EventMorsel<'a>, E> + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
        EventOf<A>: DomainEvent,
        S::Stream: Send,
    {
        let store = self.store.clone();
        let codec = Arc::<C>::clone(&self.codec);
        let root = AggregateRoot::<A>::new(id);

        let raw_stream = store
            .raw()
            .read_stream(root.id(), Version::INITIAL)
            .await
            .map_err(|e| LoadWithError::Store(StoreError::Adapter(e)))?;

        let upcast = Arc::new(upcast);
        raw_stream
            .map_err(|e| LoadWithError::Store(StoreError::Adapter(e)))
            .try_fold(root, move |mut r, env| {
                let codec = Arc::<C>::clone(&codec);
                let upcast = Arc::<F>::clone(&upcast);
                async move {
                    let version = env.version();
                    let morsel = EventMorsel::borrowed(
                        env.event_type(),
                        env.schema_version_as_version(),
                        env.payload(),
                    );
                    let transformed = upcast(morsel).map_err(LoadWithError::Upcast)?;
                    // Synthesize a fresh aligned envelope from the transformed
                    // morsel — the codec's new shape decodes from an envelope,
                    // not raw bytes, so post-upcast we rebuild the wire row.
                    let upcast_env = PersistedEnvelope::for_decode(
                        transformed.event_type(),
                        transformed.payload(),
                    )
                    .map_err(|e| LoadWithError::Store(StoreError::EnvelopeSynthesis(e)))?;
                    let event: EventOf<A> = <C as Decode<EventOf<A>>>::decode(&codec, &upcast_env)
                        .map_err(|e| LoadWithError::Store(StoreError::Decode(e)))?;
                    r.replay(version, &event)
                        .map_err(|e| LoadWithError::Store(StoreError::Kernel(e)))?;
                    Ok(r)
                }
            })
            .await
    }

    /// Persist decided events, stamping the schema version on each via
    /// `current_version`.
    ///
    /// `current_version` is typically the associated function the
    /// `#[nexus::transforms]` macro emits (e.g.
    /// `OrderTransforms::current_version`). For event types it doesn't
    /// know about, it returns `None` and the schema version falls back
    /// to [`Version::INITIAL`] (the same default as the no-upcaster
    /// [`save`](Repository::save)).
    ///
    /// # Errors
    ///
    /// The same set of errors [`save`](Repository::save) can produce —
    /// the schema-version lookup itself is infallible.
    pub async fn save_with<F, const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
        current_version: F,
    ) -> Result<
        (),
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>,
    >
    where
        A: Aggregate,
        S: RawEventStore + 'static,
        C: Encode<EventOf<A>> + Decode<EventOf<A>> + 'static,
        F: Fn(&str) -> Option<Version>,
        EventOf<A>: DomainEvent,
    {
        save_owning::<A, S, C, _, N>(&self.store, &self.codec, aggregate, events, current_version)
            .await
    }
}

// Internal save path shared between Repository::save (no upcaster, always
// stamps Version::INITIAL) and EventStore::save_with (uses the user's
// current_version fn). The owning-codec variant.
async fn save_owning<A, S, C, F, const N: usize>(
    store: &Store<S>,
    codec: &Arc<C>,
    aggregate: &mut AggregateRoot<A>,
    events: &Events<EventOf<A>, N>,
    current_version: F,
) -> Result<
    (),
    StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>,
>
where
    A: Aggregate,
    S: RawEventStore,
    C: Encode<EventOf<A>> + Decode<EventOf<A>>,
    F: Fn(&str) -> Option<Version>,
    EventOf<A>: DomainEvent,
{
    let expected_version = aggregate.version();

    let mut next_version =
        first_persisted_version(expected_version).ok_or(StoreError::VersionOverflow)?;

    let mut envelopes = Vec::with_capacity(events.len());
    // `events` is non-empty (`&Events<_, N>` guarantees >= 1), so the loop
    // runs at least once and `last_version` is always overwritten before use.
    let mut last_version = next_version;

    for event in events {
        let payload =
            <C as Encode<EventOf<A>>>::encode(codec, event).map_err(StoreError::Encode)?;

        let event_name = event.name();
        let schema_version = current_version(event_name).unwrap_or(Version::INITIAL);
        let schema_nz32 = version_to_nz32(schema_version).ok_or(StoreError::VersionOverflow)?;

        let envelope = pending_envelope(next_version)
            .event_type(event_name)
            .payload(payload)?
            .schema_version(SchemaVersion::new(schema_nz32))
            .build();

        last_version = next_version;
        envelopes.push(envelope);

        if envelopes.len() < events.len() {
            next_version = next_version.next().ok_or(StoreError::VersionOverflow)?;
        }
    }

    store
        .raw()
        .append(aggregate.id(), expected_version, &envelopes)
        .await
        .map_err(|err| match err {
            AppendError::Conflict {
                stream_id,
                expected,
                actual,
            } => StoreError::Conflict {
                stream_id,
                expected,
                actual,
            },
            AppendError::Store(e) => StoreError::Adapter(e),
        })?;

    aggregate.commit_persisted(last_version, events);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// ZeroCopyEventStore — borrowing codec (rkyv, flatbuffers, etc.)
// ═══════════════════════════════════════════════════════════════════════════

/// Event store using [`Encode`] + borrowing [`Decode`] — zero allocation on decode.
///
/// For zero-copy formats where the serialized bytes can be reinterpreted
/// in-place. The decoded `&E` borrows directly from the envelope's
/// payload bytes. Pick the borrowing shape via the
/// `Decode<E, Output<'a> = &'a E>` HRTB bound below.
///
/// # Construction
///
/// Created via [`Store::repository::<A>()`](crate::store::Store::repository),
/// which names the aggregate `A` once:
///
/// ```ignore
/// let store = Store::new(backend);
/// let orders = store.repository::<Order>().codec(OrderCodec).build_zero_copy();
/// ```
///
/// See [`EventStore`]'s doc comment for the aggregate-binding rationale (`A`
/// is a phantom type parameter so `load`/`save` infer the aggregate), the
/// upcasting story (use [`load_with`](Self::load_with) /
/// [`save_with`](Self::save_with)), and the rationale behind `Arc`-owning
/// the codec.
pub struct ZeroCopyEventStore<S, C, A> {
    store: Store<S>,
    codec: Arc<C>,
    _aggregate: PhantomData<fn() -> A>,
}

impl<S, C, A> ZeroCopyEventStore<S, C, A> {
    /// Create a zero-copy event store bound to a shared store and codec for aggregate `A`.
    pub(crate) fn new(store: Store<S>, codec: C) -> Self {
        Self {
            store,
            codec: Arc::new(codec),
            _aggregate: PhantomData,
        }
    }
}

impl<S, C, A> ReplayFrom<A> for ZeroCopyEventStore<S, C, A>
where
    A: Aggregate,
    S: RawEventStore + 'static,
    for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = &'a EventOf<A>> + 'static,
    EventOf<A>: DomainEvent,
    S::Stream: Send,
{
    type Error =
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>;

    async fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, Self::Error> {
        let store = self.store.clone();
        let codec = Arc::<C>::clone(&self.codec);

        let raw_stream = store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(StoreError::Adapter)?;

        raw_stream
            .map_err(StoreError::Adapter)
            .try_fold(root, move |mut r, env| {
                let codec = Arc::<C>::clone(&codec);
                async move {
                    let version = env.version();
                    let event: &EventOf<A> = <C as Decode<EventOf<A>>>::decode(&codec, &env)
                        .map_err(StoreError::Decode)?;
                    r.replay(version, event)?;
                    Ok(r)
                }
            })
            .await
    }
}

impl<S, C, A> Repository<A> for ZeroCopyEventStore<S, C, A>
where
    A: Aggregate,
    S: RawEventStore + 'static,
    for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = &'a EventOf<A>> + 'static,
    EventOf<A>: DomainEvent,
    S::Stream: Send,
{
    type Error =
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
        let root = AggregateRoot::<A>::new(id);
        self.replay_from(root, Version::INITIAL).await
    }

    async fn save<const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
    ) -> Result<(), Self::Error> {
        save_borrowing::<A, S, C, _, N>(&self.store, &self.codec, aggregate, events, |_| None).await
    }
}

impl<S, C, A> ZeroCopyEventStore<S, C, A> {
    /// Load an aggregate via the zero-copy decode path, running `upcast`
    /// over each persisted event before decoding.
    ///
    /// See [`EventStore::load_with`] for the function-pointer convention
    /// and the bounds rationale.
    ///
    /// # Errors
    ///
    /// Same shape as [`EventStore::load_with`] — `LoadWithError::Store`
    /// for non-upcast errors, `LoadWithError::Upcast` for the user's
    /// upcast function's failure.
    pub async fn load_with<F, E>(
        &self,
        id: A::Id,
        upcast: F,
    ) -> Result<
        AggregateRoot<A>,
        LoadWithError<
            S::Error,
            <C as Encode<EventOf<A>>>::Error,
            <C as Decode<EventOf<A>>>::Error,
            E,
        >,
    >
    where
        A: Aggregate,
        S: RawEventStore + 'static,
        for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = &'a EventOf<A>> + 'static,
        F: for<'a> Fn(EventMorsel<'a>) -> Result<EventMorsel<'a>, E> + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
        EventOf<A>: DomainEvent,
        S::Stream: Send,
    {
        let store = self.store.clone();
        let codec = Arc::<C>::clone(&self.codec);
        let root = AggregateRoot::<A>::new(id);

        let raw_stream = store
            .raw()
            .read_stream(root.id(), Version::INITIAL)
            .await
            .map_err(|e| LoadWithError::Store(StoreError::Adapter(e)))?;

        let upcast = Arc::new(upcast);
        raw_stream
            .map_err(|e| LoadWithError::Store(StoreError::Adapter(e)))
            .try_fold(root, move |mut r, env| {
                let codec = Arc::<C>::clone(&codec);
                let upcast = Arc::<F>::clone(&upcast);
                async move {
                    let version = env.version();
                    let morsel = EventMorsel::borrowed(
                        env.event_type(),
                        env.schema_version_as_version(),
                        env.payload(),
                    );
                    let transformed = upcast(morsel).map_err(LoadWithError::Upcast)?;
                    let upcast_env = PersistedEnvelope::for_decode(
                        transformed.event_type(),
                        transformed.payload(),
                    )
                    .map_err(|e| LoadWithError::Store(StoreError::EnvelopeSynthesis(e)))?;
                    let event: &EventOf<A> = <C as Decode<EventOf<A>>>::decode(&codec, &upcast_env)
                        .map_err(|e| LoadWithError::Store(StoreError::Decode(e)))?;
                    r.replay(version, event)
                        .map_err(|e| LoadWithError::Store(StoreError::Kernel(e)))?;
                    Ok(r)
                }
            })
            .await
    }

    /// Persist decided events, stamping the schema version on each via
    /// `current_version`. See [`EventStore::save_with`] for details.
    ///
    /// # Errors
    ///
    /// Same shape as [`Repository::save`] — adapter, encode, conflict,
    /// kernel, and version-overflow errors propagate through
    /// [`StoreError`]. The `current_version` lookup itself is infallible.
    pub async fn save_with<F, const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
        current_version: F,
    ) -> Result<
        (),
        StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>,
    >
    where
        A: Aggregate,
        S: RawEventStore + 'static,
        for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = &'a EventOf<A>> + 'static,
        F: Fn(&str) -> Option<Version>,
        EventOf<A>: DomainEvent,
    {
        save_borrowing::<A, S, C, _, N>(
            &self.store,
            &self.codec,
            aggregate,
            events,
            current_version,
        )
        .await
    }
}

// Internal save path for the zero-copy facade. Same structure as
// `save_owning` but with the borrowing-`Decode` shape as the decode side
// of the `StoreError` parameter pack — we only encode here, so the decode
// error type is structurally present in the `StoreError` type but never
// constructed.
async fn save_borrowing<A, S, C, F, const N: usize>(
    store: &Store<S>,
    codec: &Arc<C>,
    aggregate: &mut AggregateRoot<A>,
    events: &Events<EventOf<A>, N>,
    current_version: F,
) -> Result<
    (),
    StoreError<S::Error, <C as Encode<EventOf<A>>>::Error, <C as Decode<EventOf<A>>>::Error>,
>
where
    A: Aggregate,
    S: RawEventStore,
    for<'a> C: Encode<EventOf<A>> + Decode<EventOf<A>, Output<'a> = &'a EventOf<A>>,
    F: Fn(&str) -> Option<Version>,
    EventOf<A>: DomainEvent,
{
    let expected_version = aggregate.version();

    let mut next_version =
        first_persisted_version(expected_version).ok_or(StoreError::VersionOverflow)?;

    let mut envelopes = Vec::with_capacity(events.len());
    // `events` is non-empty (`&Events<_, N>` guarantees >= 1), so the loop
    // runs at least once and `last_version` is always overwritten before use.
    let mut last_version = next_version;

    for event in events {
        let payload =
            <C as Encode<EventOf<A>>>::encode(codec, event).map_err(StoreError::Encode)?;

        let event_name = event.name();
        let schema_version = current_version(event_name).unwrap_or(Version::INITIAL);
        let schema_nz32 = version_to_nz32(schema_version).ok_or(StoreError::VersionOverflow)?;

        let envelope = pending_envelope(next_version)
            .event_type(event_name)
            .payload(payload)?
            .schema_version(SchemaVersion::new(schema_nz32))
            .build();

        last_version = next_version;
        envelopes.push(envelope);

        if envelopes.len() < events.len() {
            next_version = next_version.next().ok_or(StoreError::VersionOverflow)?;
        }
    }

    store
        .raw()
        .append(aggregate.id(), expected_version, &envelopes)
        .await
        .map_err(|err| match err {
            AppendError::Conflict {
                stream_id,
                expected,
                actual,
            } => StoreError::Conflict {
                stream_id,
                expected,
                actual,
            },
            AppendError::Store(e) => StoreError::Adapter(e),
        })?;

    aggregate.commit_persisted(last_version, events);
    Ok(())
}

#[cfg(test)]
mod version_helper_tests {
    use super::first_persisted_version;
    use nexus::Version;

    #[test]
    fn fresh_stream_starts_at_initial() {
        assert_eq!(first_persisted_version(None), Some(Version::INITIAL));
    }

    #[test]
    fn existing_stream_advances_by_one() {
        let v = Version::INITIAL;
        assert_eq!(first_persisted_version(Some(v)), v.next());
    }

    #[test]
    fn overflow_at_max_returns_none() {
        let max = Version::new(u64::MAX).expect("u64::MAX is non-zero");
        assert_eq!(first_persisted_version(Some(max)), None);
    }
}
