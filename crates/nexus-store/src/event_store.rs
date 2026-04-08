use std::num::NonZeroU32;

use crate::borrowing_codec::BorrowingCodec;
use crate::codec::Codec;
use crate::envelope::pending_envelope;
use crate::error::StoreError;
use crate::morsel::EventMorsel;
use crate::raw::RawEventStore;
use crate::repository::Repository;
use crate::stream::EventStream;
use crate::upcaster::Upcaster;
use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

// ═══════════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a `Version` (`NonZeroU64`) to a `NonZeroU32` for the envelope's
/// `schema_version` field. Returns `Err(StoreError::Codec)` if the version
/// exceeds `u32::MAX`.
fn version_to_nz32(version: Version) -> Result<NonZeroU32, StoreError> {
    let raw = version.as_u64();
    let narrow = u32::try_from(raw)
        .map_err(|_| StoreError::Codec(format!("schema version {raw} exceeds u32::MAX").into()))?;
    // SAFETY: Version wraps NonZeroU64, so raw >= 1, so narrow >= 1.
    NonZeroU32::new(narrow)
        .ok_or_else(|| StoreError::Codec("schema version is zero (unreachable)".into()))
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
/// ```ignore
/// // No transforms:
/// let es = EventStore::new(store, codec);
///
/// // With transforms (proc-macro generated):
/// let es = EventStore::with_upcaster(store, codec, OrderTransforms);
/// ```
pub struct EventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcaster: U,
}

impl<S, C> EventStore<S, C, ()> {
    /// Create a new event store with no transforms.
    pub const fn new(store: S, codec: C) -> Self {
        Self {
            store,
            codec,
            upcaster: (),
        }
    }
}

impl<S, C, U> EventStore<S, C, U> {
    /// Create an event store with an upcaster for schema evolution.
    pub const fn with_upcaster(store: S, codec: C, upcaster: U) -> Self {
        Self {
            store,
            codec,
            upcaster,
        }
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
    type Error = StoreError;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(&id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            // Build morsel from envelope — all Cow::Borrowed (zero-alloc).
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );

            // Run through upcaster — zero-alloc when no transform fires.
            let transformed = self
                .upcaster
                .apply(morsel)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            // Decode.
            let event = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), &event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = aggregate.version();

        // Compute the first event's version.
        let mut next_version = match expected_version {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(StoreError::Codec(
                "version overflow: cannot advance past u64::MAX".into(),
            ))?,
        };

        let mut envelopes = Vec::with_capacity(events.len());

        for event in events {
            let payload = self
                .codec
                .encode(event)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            let event_name = event.name();
            let schema_version = self
                .upcaster
                .current_version(event_name)
                .unwrap_or(Version::INITIAL);
            let schema_nz32 = version_to_nz32(schema_version)?;

            let envelope = pending_envelope(next_version)
                .event_type(event_name)
                .payload(payload)
                .schema_version(schema_nz32)
                .build_without_metadata();

            envelopes.push(envelope);

            // Advance version for next event (if any).
            // Track last_version before advancing so we don't lose it.
            if envelopes.len() < events.len() {
                next_version = next_version.next().ok_or(StoreError::Codec(
                    "version overflow during batch construction".into(),
                ))?;
            }
        }

        // Append to store.
        match self
            .store
            .append(aggregate.id(), expected_version, &envelopes)
            .await
        {
            Ok(()) => {
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
            Err(crate::error::AppendError::Conflict {
                stream_id,
                expected,
                actual,
            }) => Err(StoreError::Conflict {
                stream_id,
                expected,
                actual,
            }),
            Err(crate::error::AppendError::Store(e)) => Err(StoreError::Adapter(Box::new(e))),
        }
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
/// ```ignore
/// // No transforms:
/// let es = ZeroCopyEventStore::new(store, codec);
///
/// // With transforms (proc-macro generated):
/// let es = ZeroCopyEventStore::with_upcaster(store, codec, OrderTransforms);
/// ```
pub struct ZeroCopyEventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcaster: U,
}

impl<S, C> ZeroCopyEventStore<S, C, ()> {
    /// Create a new zero-copy event store with no transforms.
    pub const fn new(store: S, codec: C) -> Self {
        Self {
            store,
            codec,
            upcaster: (),
        }
    }
}

impl<S, C, U> ZeroCopyEventStore<S, C, U> {
    /// Create a zero-copy event store with an upcaster for schema evolution.
    pub const fn with_upcaster(store: S, codec: C, upcaster: U) -> Self {
        Self {
            store,
            codec,
            upcaster,
        }
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
    type Error = StoreError;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(&id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            // Build morsel from envelope — all Cow::Borrowed (zero-alloc).
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );

            // Run through upcaster — zero-alloc when no transform fires.
            let transformed = self
                .upcaster
                .apply(morsel)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            // Decode — zero-copy: &E borrows from morsel payload.
            let event: &EventOf<A> = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = aggregate.version();

        // Compute the first event's version.
        let mut next_version = match expected_version {
            None => Version::INITIAL,
            Some(v) => v.next().ok_or(StoreError::Codec(
                "version overflow: cannot advance past u64::MAX".into(),
            ))?,
        };

        let mut envelopes = Vec::with_capacity(events.len());

        for event in events {
            let payload = self
                .codec
                .encode(event)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            let event_name = event.name();
            let schema_version = self
                .upcaster
                .current_version(event_name)
                .unwrap_or(Version::INITIAL);
            let schema_nz32 = version_to_nz32(schema_version)?;

            let envelope = pending_envelope(next_version)
                .event_type(event_name)
                .payload(payload)
                .schema_version(schema_nz32)
                .build_without_metadata();

            envelopes.push(envelope);

            // Advance version for next event (if any).
            if envelopes.len() < events.len() {
                next_version = next_version.next().ok_or(StoreError::Codec(
                    "version overflow during batch construction".into(),
                ))?;
            }
        }

        // Append to store.
        match self
            .store
            .append(aggregate.id(), expected_version, &envelopes)
            .await
        {
            Ok(()) => {
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
            Err(crate::error::AppendError::Conflict {
                stream_id,
                expected,
                actual,
            }) => Err(StoreError::Conflict {
                stream_id,
                expected,
                actual,
            }),
            Err(crate::error::AppendError::Store(e)) => Err(StoreError::Adapter(Box::new(e))),
        }
    }
}
