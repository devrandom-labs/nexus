use std::future::Future;
use std::num::NonZeroU32;

use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

use crate::codec::{BorrowingCodec, Codec};
use crate::envelope::pending_envelope;
use crate::error::{AppendError, StoreError};
use crate::store::{RawEventStore, Store};
use crate::stream::EventStreamExt;
use crate::upcasting::Upcaster;

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

// ═══════════════════════════════════════════════════════════════════════════
// EventStore — owning codec (serde, bincode, postcard, etc.)
// ═══════════════════════════════════════════════════════════════════════════

/// Event store using an owning [`Codec`] — one allocation per decoded event.
///
/// For serde-based formats where deserialization produces an owned value.
///
/// # Construction
///
/// Created via [`Store::repository()`](crate::store::Store::repository):
///
/// ```ignore
/// let store = Store::new(backend);
///
/// // No transforms:
/// let orders = store.repository().codec(OrderCodec).build();
///
/// // With transforms:
/// let orders = store.repository().codec(OrderCodec).upcaster(OrderTransforms).build();
/// ```
pub struct EventStore<S, C, U = ()> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}

impl<S, C, U> EventStore<S, C, U> {
    /// Create an event store bound to a shared store, codec, and upcaster.
    pub(crate) const fn new(store: Store<S>, codec: C, upcaster: U) -> Self {
        Self {
            store,
            codec,
            upcaster,
        }
    }
}

impl<A, S, C, U> ReplayFrom<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: Upcaster,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError<S::Error, C::Error, U::Error>;

    async fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, Self::Error> {
        let stream = self
            .store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(StoreError::Adapter)?;

        stream
            .decoder()
            .codec(&self.codec)
            .upcaster(&self.upcaster)
            .build()
            .try_fold(root, |mut root, version, event| {
                root.replay(version, &event)?;
                Ok(root)
            })
            .await
    }
}

impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: Upcaster,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError<S::Error, C::Error, U::Error>;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
        let root = AggregateRoot::<A>::new(id);
        self.replay_from(root, Version::INITIAL).await
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), Self::Error> {
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = aggregate.version();

        // Compute the first event's version.
        let mut next_version = match expected_version {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(StoreError::VersionOverflow)?,
        };

        let mut envelopes = Vec::with_capacity(events.len());

        for event in events {
            let payload = self.codec.encode(event).map_err(StoreError::Codec)?;

            let event_name = event.name();
            let schema_version = self
                .upcaster
                .current_version(event_name)
                .unwrap_or(Version::INITIAL);
            let schema_nz32 = version_to_nz32(schema_version).ok_or(StoreError::VersionOverflow)?;

            let envelope = pending_envelope(next_version)
                .event_type(event_name)
                .payload(payload)
                .schema_version(schema_nz32)
                .build_without_metadata();

            envelopes.push(envelope);

            // Advance version for next event (if any).
            // Track last_version before advancing so we don't lose it.
            if envelopes.len() < events.len() {
                next_version = next_version.next().ok_or(StoreError::VersionOverflow)?;
            }
        }

        // Append to store.
        self.store
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

        // The last envelope's version is the new aggregate version.
        #[allow(
            clippy::expect_used,
            reason = "envelopes is non-empty: checked events.is_empty() above"
        )]
        let last_version = envelopes.last().expect("envelopes is non-empty").version();

        aggregate.advance_version(last_version);
        for event in events {
            aggregate.apply_event(event);
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ZeroCopyEventStore — borrowing codec (rkyv, flatbuffers, etc.)
// ═══════════════════════════════════════════════════════════════════════════

/// Event store using a [`BorrowingCodec`] — zero allocation on decode.
///
/// For zero-copy formats where the serialized bytes can be reinterpreted
/// in-place. The decoded `&E` borrows directly from the cursor buffer.
///
/// # Construction
///
/// Created via [`Store::repository()`](crate::store::Store::repository):
///
/// ```ignore
/// let store = Store::new(backend);
///
/// // No transforms:
/// let orders = store.repository().codec(OrderCodec).build_zero_copy();
///
/// // With transforms:
/// let orders = store.repository().codec(OrderCodec).upcaster(OrderTransforms).build_zero_copy();
/// ```
pub struct ZeroCopyEventStore<S, C, U = ()> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}

impl<S, C, U> ZeroCopyEventStore<S, C, U> {
    /// Create a zero-copy event store bound to a shared store, codec, and upcaster.
    pub(crate) const fn new(store: Store<S>, codec: C, upcaster: U) -> Self {
        Self {
            store,
            codec,
            upcaster,
        }
    }
}

impl<A, S, C, U> ReplayFrom<A> for ZeroCopyEventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
    U: Upcaster,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError<S::Error, C::Error, U::Error>;

    async fn replay_from(
        &self,
        root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, Self::Error> {
        let stream = self
            .store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(StoreError::Adapter)?;

        stream
            .decoder()
            .borrowing_codec(&self.codec)
            .upcaster(&self.upcaster)
            .build()
            .try_fold(root, |mut root, version, event: &EventOf<A>| {
                root.replay(version, event)?;
                Ok(root)
            })
            .await
    }
}

impl<A, S, C, U> Repository<A> for ZeroCopyEventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
    U: Upcaster,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError<S::Error, C::Error, U::Error>;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
        let root = AggregateRoot::<A>::new(id);
        self.replay_from(root, Version::INITIAL).await
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), Self::Error> {
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = aggregate.version();

        // Compute the first event's version.
        let mut next_version = match expected_version {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(StoreError::VersionOverflow)?,
        };

        let mut envelopes = Vec::with_capacity(events.len());

        for event in events {
            let payload = self.codec.encode(event).map_err(StoreError::Codec)?;

            let event_name = event.name();
            let schema_version = self
                .upcaster
                .current_version(event_name)
                .unwrap_or(Version::INITIAL);
            let schema_nz32 = version_to_nz32(schema_version).ok_or(StoreError::VersionOverflow)?;

            let envelope = pending_envelope(next_version)
                .event_type(event_name)
                .payload(payload)
                .schema_version(schema_nz32)
                .build_without_metadata();

            envelopes.push(envelope);

            // Advance version for next event (if any).
            if envelopes.len() < events.len() {
                next_version = next_version.next().ok_or(StoreError::VersionOverflow)?;
            }
        }

        // Append to store.
        self.store
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

        #[allow(
            clippy::expect_used,
            reason = "envelopes is non-empty: checked events.is_empty() above"
        )]
        let last_version = envelopes.last().expect("envelopes is non-empty").version();

        aggregate.advance_version(last_version);
        for event in events {
            aggregate.apply_event(event);
        }
        Ok(())
    }
}
