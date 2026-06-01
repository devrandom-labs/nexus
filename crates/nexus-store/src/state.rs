use std::future::Future;
use std::num::{NonZeroU32, NonZeroU64};

use nexus::{Id, Version};

use crate::codec::{Decode, Encode};

// ═══════════════════════════════════════════════════════════════════════════
// SnapshotStore<S, P> — atomic state + position persistence
// ═══════════════════════════════════════════════════════════════════════════

/// Atomic persistence of a snapshot — derived state plus the position it
/// was folded up to.
///
/// One trait, two callers:
/// - aggregate snapshots — the aggregate's state, at its `Version`.
/// - projections — the projection's state, at its position.
///
/// State and position are saved and loaded *together*. A half-write
/// (state without position, or position without state) is impossible:
/// the trait exposes only the two *combined* operations, never "save
/// state alone". Atomicity itself is the adapter's responsibility — it
/// owns both the state and position storage and commits them in one
/// transaction.
///
/// Generic over the position type `P` so one trait serves a single
/// stream (`P = Version`) and a multi-stream, single-producer projection
/// (`P = GlobalSeq`).
pub trait SnapshotStore<S, P>: Send + Sync {
    /// Adapter-specific error type.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Load the saved state and position from a single consistent snapshot.
    ///
    /// Returns `None` if nothing has been saved for `id`, or if the saved
    /// state's schema version does not match `schema_version`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying store fails to read.
    fn hydrate(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<(P, S)>, Self::Error>> + Send;

    /// Save state and position together, in a single transaction.
    ///
    /// Either both are durably stored, or neither is.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying store fails to commit.
    fn commit(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
        position: P,
        state: &S,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<S, P, T> SnapshotStore<S, P> for &T
where
    S: Send + Sync,
    P: Send,
    T: SnapshotStore<S, P>,
{
    type Error = T::Error;

    fn hydrate(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<(P, S)>, Self::Error>> + Send {
        (**self).hydrate(id, schema_version)
    }

    fn commit(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
        position: P,
        state: &S,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        (**self).commit(id, schema_version, position, state)
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
// CodecSnapshotStore<SS, C> — byte-level <-> typed bridge via Encode + Decode
// ═══════════════════════════════════════════════════════════════════════════

/// Adapter that bridges a byte-level [`SnapshotStore<Vec<u8>, P>`] to a typed
/// [`SnapshotStore<S, P>`] by encoding/decoding through an [`Encode<S>`] +
/// [`Decode<S>`] pair.
///
/// Use this when your storage backend works with raw bytes (e.g., fjall)
/// but consumers need typed state. The position `P` is opaque to the
/// bridge — it passes through untouched.
pub struct CodecSnapshotStore<SS, C> {
    store: SS,
    codec: C,
}

impl<SS, C> CodecSnapshotStore<SS, C> {
    /// Create a new codec-bridged snapshot store.
    #[must_use]
    pub const fn new(store: SS, codec: C) -> Self {
        Self { store, codec }
    }
}

impl<S, P, SS, C> SnapshotStore<S, P> for CodecSnapshotStore<SS, C>
where
    S: Send + Sync + 'static,
    P: Send,
    SS: SnapshotStore<Vec<u8>, P>,
    C: Encode<S> + Decode<S>,
{
    type Error =
        CodecSnapshotStoreError<SS::Error, <C as Encode<S>>::Error, <C as Decode<S>>::Error>;

    async fn hydrate(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<(P, S)>, Self::Error> {
        let Some((position, bytes)) = self
            .store
            .hydrate(id, schema_version)
            .await
            .map_err(CodecSnapshotStoreError::Store)?
        else {
            return Ok(None);
        };

        let label = id.to_label();
        let state = <C as Decode<S>>::decode(&self.codec, &label, &bytes)
            .map_err(CodecSnapshotStoreError::Decode)?;

        Ok(Some((position, state)))
    }

    async fn commit(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
        position: P,
        state: &S,
    ) -> Result<(), Self::Error> {
        let bytes = <C as Encode<S>>::encode(&self.codec, state)
            .map_err(CodecSnapshotStoreError::Encode)?;

        // SnapshotStore<Vec<u8>, P> requires &Vec<u8>; adapt by copying.
        // Snapshot writes are rare relative to the read path, so the
        // extra allocation here is acceptable.
        let bytes_vec = bytes.to_vec();
        self.store
            .commit(id, schema_version, position, &bytes_vec)
            .await
            .map_err(CodecSnapshotStoreError::Store)
    }
}

/// Error from [`CodecSnapshotStore`] — the underlying store, the encoder, or the decoder.
#[derive(Debug, thiserror::Error)]
pub enum CodecSnapshotStoreError<S, EncErr, DecErr> {
    /// The underlying byte-level store failed.
    #[error(transparent)]
    Store(S),
    /// Encoding failed.
    #[error(transparent)]
    Encode(EncErr),
    /// Decoding failed.
    #[error(transparent)]
    Decode(DecErr),
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

    use super::SnapshotStore;

    /// In-memory snapshot store for tests.
    #[derive(Debug, Default)]
    pub struct InMemorySnapshotStore<S, P> {
        snapshots: RwLock<HashMap<String, (NonZeroU32, P, S)>>,
    }

    impl<S, P> InMemorySnapshotStore<S, P> {
        #[must_use]
        pub fn new() -> Self {
            Self {
                snapshots: RwLock::new(HashMap::new()),
            }
        }
    }

    impl<S, P> SnapshotStore<S, P> for InMemorySnapshotStore<S, P>
    where
        S: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        type Error = Infallible;

        async fn hydrate(
            &self,
            id: &impl Id,
            schema_version: NonZeroU32,
        ) -> Result<Option<(P, S)>, Infallible> {
            let snapshots = self.snapshots.read().await;
            Ok(snapshots
                .get(&id.to_string())
                .filter(|(stored_schema, _, _)| *stored_schema == schema_version)
                .map(|(_, position, state)| (position.clone(), state.clone())))
        }

        async fn commit(
            &self,
            id: &impl Id,
            schema_version: NonZeroU32,
            position: P,
            state: &S,
        ) -> Result<(), Infallible> {
            self.snapshots
                .write()
                .await
                .insert(id.to_string(), (schema_version, position, state.clone()));
            Ok(())
        }
    }
}

#[cfg(feature = "testing")]
pub use testing::InMemorySnapshotStore;
