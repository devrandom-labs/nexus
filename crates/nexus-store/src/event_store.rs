use crate::borrowing_codec::BorrowingCodec;
use crate::codec::Codec;
use crate::envelope::pending_envelope;
use crate::error::{AppendError, StoreError, UpcastError};
use crate::raw::RawEventStore;
use crate::repository::Repository;
use crate::stream::EventStream;
use crate::upcaster_chain::{Chain, UpcasterChain};
use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

// ─── Shared upcaster logic ───────────────────────────────────────────────

const UPCAST_CHAIN_LIMIT: u32 = 100;

fn run_upcasters<U: UpcasterChain>(
    upcasters: &U,
    event_type: &str,
    schema_version: u32,
    payload: &[u8],
) -> Result<(String, u32, Vec<u8>), StoreError> {
    let mut current_type = event_type.to_owned();
    let mut current_version = schema_version;
    let mut current_payload = payload.to_vec();
    let mut iterations = 0u32;

    while let Some((new_type, new_version, new_payload)) =
        upcasters.try_upcast(&current_type, current_version, &current_payload)
    {
        iterations += 1;
        if iterations > UPCAST_CHAIN_LIMIT {
            return Err(StoreError::Codec(Box::new(
                UpcastError::ChainLimitExceeded {
                    event_type: current_type,
                    schema_version: current_version,
                    limit: UPCAST_CHAIN_LIMIT,
                },
            )));
        }
        if new_version <= current_version {
            return Err(StoreError::Codec(Box::new(
                UpcastError::VersionNotAdvanced {
                    event_type: current_type,
                    input_version: current_version,
                    output_version: new_version,
                },
            )));
        }
        if new_type.is_empty() {
            return Err(StoreError::Codec(Box::new(UpcastError::EmptyEventType {
                input_event_type: current_type,
                schema_version: new_version,
            })));
        }
        current_type = new_type;
        current_version = new_version;
        current_payload = new_payload;
    }

    Ok((current_type, current_version, current_payload))
}

/// Determine the "current" schema version for an event type by probing
/// the upcaster chain. Walks versions from 1 upward until no upcaster
/// matches. New events should be stored at this version so that
/// upcasters skip them on load.
fn current_schema_version<U: UpcasterChain>(upcasters: &U, event_type: &str) -> u32 {
    let mut version = 1u32;
    while upcasters.can_upcast(event_type, version) {
        version = version.saturating_add(1);
        if version > UPCAST_CHAIN_LIMIT + 1 {
            break;
        }
    }
    version
}

// ─── Shared save logic ───────────────────────────────────────────────────

#[allow(clippy::expect_used, reason = "checked non-empty above")]
async fn save_with_encoder<A, S, U, F>(
    store: &S,
    upcasters: &U,
    encode_fn: F,
    stream_id: &str,
    aggregate: &mut AggregateRoot<A>,
) -> Result<(), StoreError>
where
    A: Aggregate,
    S: RawEventStore,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
    F: Fn(&EventOf<A>) -> Result<Vec<u8>, StoreError>,
{
    let events = aggregate.uncommitted_events();
    if events.is_empty() {
        return Ok(());
    }

    let expected_version = Version::from_persisted(
        events
            .first()
            .expect("checked non-empty above")
            .version()
            .as_u64()
            - 1,
    );

    let mut envelopes = Vec::with_capacity(events.len());
    for ve in events {
        let payload = encode_fn(ve.event())?;
        let sv = current_schema_version(upcasters, ve.event().name());
        envelopes.push(
            pending_envelope(stream_id.into())
                .version(ve.version())
                .event_type(ve.event().name())
                .payload(payload)
                .schema_version(sv)
                .build_without_metadata(),
        );
    }

    match store.append(stream_id, expected_version, &envelopes).await {
        Ok(()) => {
            aggregate.clear_uncommitted();
            Ok(())
        }
        Err(AppendError::Conflict {
            stream_id: sid,
            expected,
            actual,
        }) => Err(StoreError::Conflict {
            stream_id: sid,
            expected,
            actual,
        }),
        Err(AppendError::Store(e)) => Err(StoreError::Adapter(Box::new(e))),
    }
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
/// let es = EventStore::new(store, codec)
///     .with_upcaster(V1ToV2)
///     .with_upcaster(V2ToV3);
/// ```
pub struct EventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcasters: U,
}

impl<S, C> EventStore<S, C, ()> {
    /// Create a new event store with no upcasters.
    pub const fn new(store: S, codec: C) -> Self {
        Self {
            store,
            codec,
            upcasters: (),
        }
    }
}

impl<S, C, U> EventStore<S, C, U> {
    /// Add an upcaster to the chain. Returns a new `EventStore` with
    /// the upcaster prepended (checked first during reads).
    pub fn with_upcaster<H>(self, upcaster: H) -> EventStore<S, C, Chain<H, U>> {
        EventStore {
            store: self.store,
            codec: self.codec,
            upcasters: Chain(upcaster, self.upcasters),
        }
    }
}

impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError;

    async fn load(&self, stream_id: &str, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(stream_id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            let (event_type, _schema_version, payload) = run_upcasters(
                &self.upcasters,
                env.event_type(),
                env.schema_version(),
                env.payload(),
            )?;

            let event = self
                .codec
                .decode(&event_type, &payload)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), &event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> Result<(), StoreError> {
        save_with_encoder(
            &self.store,
            &self.upcasters,
            |event| {
                self.codec
                    .encode(event)
                    .map_err(|e| StoreError::Codec(Box::new(e)))
            },
            stream_id,
            aggregate,
        )
        .await
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
/// let es = ZeroCopyEventStore::new(store, codec)
///     .with_upcaster(V1ToV2);
/// ```
pub struct ZeroCopyEventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcasters: U,
}

impl<S, C> ZeroCopyEventStore<S, C, ()> {
    /// Create a new zero-copy event store with no upcasters.
    pub const fn new(store: S, codec: C) -> Self {
        Self {
            store,
            codec,
            upcasters: (),
        }
    }
}

impl<S, C, U> ZeroCopyEventStore<S, C, U> {
    /// Add an upcaster to the chain.
    pub fn with_upcaster<H>(self, upcaster: H) -> ZeroCopyEventStore<S, C, Chain<H, U>> {
        ZeroCopyEventStore {
            store: self.store,
            codec: self.codec,
            upcasters: Chain(upcaster, self.upcasters),
        }
    }
}

impl<A, S, C, U> Repository<A> for ZeroCopyEventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
    for<'a> S::Stream<'a>: Send,
{
    type Error = StoreError;

    async fn load(&self, stream_id: &str, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(stream_id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            let (event_type, _schema_version, payload) = run_upcasters(
                &self.upcasters,
                env.event_type(),
                env.schema_version(),
                env.payload(),
            )?;

            let event: &EventOf<A> = self
                .codec
                .decode(&event_type, &payload)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> Result<(), StoreError> {
        save_with_encoder(
            &self.store,
            &self.upcasters,
            |event| {
                self.codec
                    .encode(event)
                    .map_err(|e| StoreError::Codec(Box::new(e)))
            },
            stream_id,
            aggregate,
        )
        .await
    }
}
