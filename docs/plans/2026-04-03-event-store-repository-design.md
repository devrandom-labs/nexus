# EventStore Repository Design

**Goal:** A concrete `Repository<A>` implementation that composes `RawEventStore` + `Codec` + `UpcasterChain` with zero-copy reads, fully static dispatch, and compile-time type safety.

**Decisions made:**
- Two codec traits: owning (`Codec<E>`) and borrowing (`BorrowingCodec<E>`)
- Heterogeneous cons-list for upcaster chain (no `dyn`, no `Vec`, no arity limit)
- No external dependencies for the HList — ~15 lines of code
- `EventStore<S, C, U>` struct with two `impl Repository<A>` blocks (one per codec flavor)

---

## Codec Traits

### `Codec<E>` (existing — unchanged)

Owning decode. For serde_json, bincode, postcard, etc.

```rust
trait Codec<E>: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
```

One allocation per decode (the owned `E`).

### `BorrowingCodec<E>` (new)

Zero-copy decode. For rkyv, flatbuffers, capnproto, etc.

```rust
trait BorrowingCodec<E: ?Sized>: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;
    fn decode<'a>(&self, event_type: &str, payload: &'a [u8]) -> Result<&'a E, Self::Error>;
}
```

`E: ?Sized` allows `E = Archived<MyEvent>` (which may be unsized).
`decode` returns `&'a E` borrowing directly from the payload buffer.
Zero allocation — pointer validation + cast only.

**rkyv usage pattern:** The aggregate's `Event` associated type is `Archived<MyEvent>`. The `AggregateState::apply(self, &Archived<MyEvent>) -> Self` works directly on archived data. No deserialization ever happens.

---

## Upcaster Chain

### Cons-list types

```rust
/// A link in the upcaster chain. Head is applied before tail.
struct Chain<H, T>(H, T);

/// Trait for compile-time upcaster chains.
trait UpcasterChain: Send + Sync {
    /// Try each upcaster in the chain. Returns None if no upcaster matches.
    /// Applies the FIRST matching upcaster only (caller loops until stable).
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)>;
}

impl UpcasterChain for () {
    fn try_upcast(&self, _: &str, _: u32, _: &[u8]) -> Option<(String, u32, Vec<u8>)> {
        None
    }
}

impl<H: EventUpcaster, T: UpcasterChain> UpcasterChain for Chain<H, T> {
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)> {
        if self.0.can_upcast(event_type, schema_version) {
            Some(self.0.upcast(event_type, schema_version, payload))
        } else {
            self.1.try_upcast(event_type, schema_version, payload)
        }
    }
}
```

### Builder ergonomics

```rust
EventStore::new(store, codec)           // U = ()
    .with_upcaster(V1ToV2)             // U = Chain<V1ToV2, ()>
    .with_upcaster(V2ToV3)             // U = Chain<V2ToV3, Chain<V1ToV2, ()>>
```

Each call returns a new `EventStore` with a different `U` type. Fully inlined by the compiler.

### Validation

The chain runs in a loop until no upcaster matches (stable). After each step:
- Validate `new_schema_version > old_schema_version` (reject stale/downgrade)
- Validate `event_type` is non-empty
- Bounded by a const limit (100) as defense-in-depth

This validation lives in `EventStore`'s load path, not in `UpcasterChain` itself — the chain is just dispatch, the facade owns the safety policy.

---

## EventStore Struct

```rust
pub struct EventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcasters: U,
}
```

Three type parameters, all monomorphized. `U` defaults to `()` (no upcasters).

### Construction

```rust
impl<S, C> EventStore<S, C, ()> {
    pub fn new(store: S, codec: C) -> Self {
        Self { store, codec, upcasters: () }
    }
}

impl<S, C, U> EventStore<S, C, U> {
    pub fn with_upcaster<H: EventUpcaster>(self, upcaster: H) -> EventStore<S, C, Chain<H, U>> {
        EventStore {
            store: self.store,
            codec: self.codec,
            upcasters: Chain(upcaster, self.upcasters),
        }
    }
}
```

### Repository impl — owning codec

```rust
impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: UpcasterChain,
{
    type Error = StoreError;

    async fn load(&self, stream_id: &str, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self.store.read_stream(stream_id, Version::INITIAL).await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            // Upcast if needed (may allocate new payload)
            let (event_type, _version, payload) = self.run_upcasters(
                env.event_type(), env.schema_version(), env.payload()
            )?;

            let event = self.codec.decode(&event_type, &payload)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), &event)?;
        }

        Ok(root)
    }

    async fn save(&self, stream_id: &str, aggregate: &mut AggregateRoot<A>) -> Result<(), StoreError> {
        let events = aggregate.take_uncommitted_events();
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = aggregate.version();
        // ^ version() already advanced by take_uncommitted_events()
        // We need the version BEFORE take. That's version - events.len().
        // Actually: take advances version to current_version.
        // The expected_version for append is the version the store has,
        // which is the version BEFORE we applied these events.
        // After take: aggregate.version() == old version + events.len()
        // So: expected = aggregate.version() - events.len()
        // But Version doesn't support subtraction... we need to track this.

        // The expected_version is the first event's version - 1
        let expected = Version::from_persisted(
            events.first().unwrap().version().as_u64() - 1
        );

        let mut envelopes = Vec::with_capacity(events.len());
        for ve in &events {
            let (version, event) = (ve.version(), ve.event());
            let payload = self.codec.encode(event)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            envelopes.push(
                pending_envelope(stream_id.into())
                    .version(version)
                    .event_type(event.name())
                    .payload(payload)
                    .build_without_metadata()
            );
        }

        self.store.append(stream_id, expected, &envelopes).await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        Ok(())
    }
}
```

### Repository impl — borrowing codec

Same structure, but `decode` returns `&'a E` borrowing from `payload`:

```rust
impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
    U: UpcasterChain,
{
    // ... same as above but decode returns &E, no allocation
}
```

The two impls don't conflict because `Codec<E>` and `BorrowingCodec<E>` are different traits — a type implements at most one.

### Upcaster validation in load path

```rust
impl<S, C, U: UpcasterChain> EventStore<S, C, U> {
    fn run_upcasters(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Result<(String, u32, Vec<u8>), StoreError> {
        let mut current_type = event_type.to_owned();
        let mut current_version = schema_version;
        let mut current_payload = payload.to_vec();
        let mut iterations = 0u32;

        while let Some((new_type, new_version, new_payload)) =
            self.upcasters.try_upcast(&current_type, current_version, &current_payload)
        {
            iterations += 1;
            if iterations > 100 {
                return Err(StoreError::Codec(Box::new(UpcastError::ChainLimitExceeded {
                    event_type: current_type,
                    schema_version: current_version,
                    limit: 100,
                })));
            }
            if new_version <= current_version {
                return Err(StoreError::Codec(Box::new(UpcastError::VersionNotAdvanced {
                    event_type: current_type,
                    input_version: current_version,
                    output_version: new_version,
                })));
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
}
```

**Optimization:** If `U = ()`, `try_upcast` is a no-op that returns `None`. The compiler eliminates the entire while loop + allocations via dead code elimination. Zero cost when no upcasters are configured.

---

## Load path: allocation profile

| Step | With BorrowingCodec | With Codec |
|------|:---:|:---:|
| Cursor buffer | reused (0 alloc) | reused (0 alloc) |
| Upcaster (no match) | 0 alloc (loop eliminated) | 0 alloc |
| Upcaster (match) | 1 alloc (new payload) | 1 alloc |
| Decode | 0 alloc (pointer cast) | 1 alloc (owned E) |
| replay() | 0 alloc (by-value apply) | 0 alloc |
| **Per-event total** | **0 alloc (happy path)** | **1 alloc** |

---

## File layout

All in `crates/nexus-store/src/`:

```
src/
├── codec.rs              ← Codec<E> (existing, unchanged)
├── borrowing_codec.rs    ← BorrowingCodec<E> (new, ~15 lines)
├── upcaster_chain.rs     ← Chain<H,T>, UpcasterChain trait (new, ~40 lines)
├── event_store.rs        ← EventStore<S,C,U>, both Repository impls (new, ~150 lines)
├── repository.rs         ← Repository<A> trait (existing, unchanged)
├── lib.rs                ← re-exports (update)
└── ... (rest unchanged)
```

---

## Open question: save path expected_version

`take_uncommitted_events()` advances the aggregate's persisted version. So after take, `aggregate.version()` reflects the NEW version (post-events). But `append()` needs the OLD version (pre-events) as `expected_version`.

Two options:
- **A)** Compute from first event: `events[0].version().as_u64() - 1` (what the design shows above)
- **B)** Capture `aggregate.version()` BEFORE calling take

Option A is simpler — the events carry their versions, and the expected_version is always `first_event_version - 1`. No need to capture state before take.
