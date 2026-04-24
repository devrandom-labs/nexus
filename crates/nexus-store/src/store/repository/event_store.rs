use std::num::NonZeroU32;

use super::replay::ReplayFrom;
use super::repository::Repository;
use crate::codec::Codec;
use crate::envelope::pending_envelope;
use crate::error::{AppendError, StoreError};
use crate::store::raw::RawEventStore;
use crate::store::store::Store;
use crate::store::stream::EventStream;
use crate::upcasting::{EventMorsel, Upcaster};
use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

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
/// Created via [`Store::repository()`](crate::store::store::Store::repository):
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
        mut root: AggregateRoot<A>,
        from: Version,
    ) -> Result<AggregateRoot<A>, Self::Error> {
        let mut stream = self
            .store
            .raw()
            .read_stream(root.id(), from)
            .await
            .map_err(StoreError::Adapter)?;

        while let Some(result) = stream.next().await {
            let env = result.map_err(StoreError::Adapter)?;

            // Build morsel from envelope — all Cow::Borrowed (zero-alloc).
            let morsel = EventMorsel::borrowed(
                env.event_type(),
                env.schema_version_as_version(),
                env.payload(),
            );

            // Run through upcaster — zero-alloc when no transform fires.
            let transformed = self.upcaster.apply(morsel).map_err(StoreError::Upcast)?;

            // Decode.
            let event = self
                .codec
                .decode(transformed.event_type(), transformed.payload())
                .map_err(StoreError::Codec)?;

            root.replay(env.version(), &event)?;
        }

        Ok(root)
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
