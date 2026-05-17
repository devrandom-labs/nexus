use std::convert::Infallible;
use std::future::Future;
use std::num::{NonZeroU32, NonZeroU64};

use nexus::{Id, Version};

use crate::codec::Codec;

/// Versioned state payload — used for both read and write paths.
///
/// Generic over `S` — the adapter decides serialization format.
/// For byte-level adapters (fjall): `State<Vec<u8>>`.
/// For typed adapters (postgres): `State<MyDomainState>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> State<S> {
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, state: S) -> Self {
        Self {
            version,
            schema_version,
            state,
        }
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    #[must_use]
    pub fn into_parts(self) -> (Version, NonZeroU32, S) {
        (self.version, self.schema_version, self.state)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// StateStore<S> — versioned state persistence trait
// ═══════════════════════════════════════════════════════════════════════════

/// Versioned state persistence trait.
///
/// Unified trait for both projection state and aggregate snapshots.
/// Generic over `S` — the adapter decides how to serialize.
///
/// `()` is the no-op implementation: `load` returns `None`,
/// `save` and `delete` silently discard.
pub trait StateStore<S>: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load persisted state, filtering by schema version.
    ///
    /// Returns `None` if no state exists or schema version mismatches.
    fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<State<S>>, Self::Error>> + Send;

    /// Persist state (overwrites existing).
    fn save(
        &self,
        id: &impl Id,
        state: &State<S>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete persisted state (idempotent).
    fn delete(&self, id: &impl Id) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<S: Send + Sync, T: StateStore<S>> StateStore<S> for &T {
    type Error = T::Error;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<State<S>>, Self::Error> {
        (**self).load(id, schema_version).await
    }

    async fn save(&self, id: &impl Id, state: &State<S>) -> Result<(), Self::Error> {
        (**self).save(id, state).await
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        (**self).delete(id).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — state persistence disabled
// ═══════════════════════════════════════════════════════════════════════════

impl<S: Send + Sync> StateStore<S> for () {
    type Error = Infallible;

    async fn load(
        &self,
        _id: &impl Id,
        _schema_version: NonZeroU32,
    ) -> Result<Option<State<S>>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _state: &State<S>) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// PersistTrigger — when-to-persist policy
// ═══════════════════════════════════════════════════════════════════════════

/// Strategy for deciding when to persist state.
///
/// Used by both projection runners (when to checkpoint projection state)
/// and snapshot decorators (when to snapshot aggregate state).
pub trait PersistTrigger: Send + Sync {
    /// Whether state should be persisted now.
    ///
    /// - `old_version`: version before the operation (`None` for first run)
    /// - `new_version`: version after the operation
    /// - `event_names`: names of events just processed
    fn should_persist(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Persist every N events (bucket-crossing algorithm).
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl PersistTrigger for EveryNEvents {
    fn should_persist(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        _event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        let n = self.0.get();
        let old_bucket = old_version.map_or(0, |v| v.as_u64() / n);
        let new_bucket = new_version.as_u64() / n;
        new_bucket > old_bucket
    }
}

/// Persist after specific event types.
#[derive(Debug, Clone)]
pub struct AfterEventTypes {
    types: Vec<&'static str>,
}

impl AfterEventTypes {
    /// Create a trigger that fires when any of the given event types is persisted.
    #[must_use]
    pub fn new(types: &[&'static str]) -> Self {
        Self {
            types: types.to_vec(),
        }
    }
}

impl PersistTrigger for AfterEventTypes {
    fn should_persist(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        mut event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CodecStateStore<SS, C> — byte-level <-> typed bridge via Codec<S>
// ═══════════════════════════════════════════════════════════════════════════

/// Adapter that bridges a byte-level [`StateStore<Vec<u8>>`] to a typed
/// [`StateStore<S>`] by encoding/decoding through a [`Codec<S>`].
///
/// Use this when your storage backend works with raw bytes (e.g., fjall)
/// but consumers need typed state.
///
/// # Example
///
/// ```ignore
/// let byte_store: impl StateStore<Vec<u8>> = fjall_store;
/// let typed_store = CodecStateStore::new(byte_store, JsonCodec::default());
/// // typed_store: impl StateStore<MyState>
/// ```
pub struct CodecStateStore<SS, C> {
    store: SS,
    codec: C,
}

impl<SS, C> CodecStateStore<SS, C> {
    /// Create a new codec-bridged state store.
    #[must_use]
    pub const fn new(store: SS, codec: C) -> Self {
        Self { store, codec }
    }
}

impl<S, SS, C> StateStore<S> for CodecStateStore<SS, C>
where
    S: Send + Sync + 'static,
    SS: StateStore<Vec<u8>>,
    C: Codec<S>,
{
    type Error = CodecStateStoreError<SS::Error, C::Error>;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<State<S>>, Self::Error> {
        let Some(raw) = self
            .store
            .load(id, schema_version)
            .await
            .map_err(CodecStateStoreError::Store)?
        else {
            return Ok(None);
        };

        let (version, schema_ver, bytes) = raw.into_parts();
        let label = id.to_label();
        let state = self
            .codec
            .decode(&label, &bytes)
            .map_err(CodecStateStoreError::Codec)?;

        Ok(Some(State::new(version, schema_ver, state)))
    }

    async fn save(&self, id: &impl Id, state: &State<S>) -> Result<(), Self::Error> {
        let bytes = self
            .codec
            .encode(state.state())
            .map_err(CodecStateStoreError::Codec)?;

        let raw = State::new(state.version(), state.schema_version(), bytes);
        self.store
            .save(id, &raw)
            .await
            .map_err(CodecStateStoreError::Store)
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        self.store
            .delete(id)
            .await
            .map_err(CodecStateStoreError::Store)
    }
}

/// Error from [`CodecStateStore`] — either the underlying store or the codec.
#[derive(Debug, thiserror::Error)]
pub enum CodecStateStoreError<S, C> {
    /// The underlying byte-level store failed.
    #[error(transparent)]
    Store(S),
    /// Encoding or decoding failed.
    #[error(transparent)]
    Codec(C),
}

// ═══════════════════════════════════════════════════════════════════════════
// In-memory testing fake (feature-gated)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "testing")]
mod testing {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::num::NonZeroU32;

    use nexus::Id;
    use tokio::sync::RwLock;

    use super::{State, StateStore};

    /// In-memory state store for tests.
    #[derive(Debug, Default)]
    pub struct InMemoryStateStore<S> {
        states: RwLock<HashMap<String, State<S>>>,
    }

    impl<S> InMemoryStateStore<S> {
        #[must_use]
        pub fn new() -> Self {
            Self {
                states: RwLock::new(HashMap::new()),
            }
        }
    }

    impl<S: Clone + Send + Sync + 'static> StateStore<S> for InMemoryStateStore<S> {
        type Error = Infallible;

        async fn load(
            &self,
            id: &impl Id,
            schema_version: NonZeroU32,
        ) -> Result<Option<State<S>>, Infallible> {
            let states = self.states.read().await;
            let key = id.to_string();
            Ok(states
                .get(&key)
                .filter(|s| s.schema_version() == schema_version)
                .cloned())
        }

        async fn save(&self, id: &impl Id, state: &State<S>) -> Result<(), Infallible> {
            let key = id.to_string();
            self.states.write().await.insert(key, state.clone());
            Ok(())
        }

        async fn delete(&self, id: &impl Id) -> Result<(), Infallible> {
            self.states.write().await.remove(&id.to_string());
            Ok(())
        }
    }
}

#[cfg(feature = "testing")]
pub use testing::InMemoryStateStore;
