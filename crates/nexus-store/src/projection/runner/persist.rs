use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::{Id, Version};

use crate::codec::Codec;
use crate::projection::pending::PendingState;
use crate::projection::store::StateStore;

use super::error::StatePersistError;

/// Polymorphic state persistence — either disabled or enabled with a store + codec.
///
/// This trait exists to solve a type-system problem: when state persistence is
/// disabled (`StateStore = ()`), we need a no-op codec. But `Codec<T> for ()`
/// is impossible in safe Rust (can't construct arbitrary `T` in `decode`).
/// Instead, `NoStatePersistence` implements this trait with `Error = Infallible`
/// and never touches any codec.
pub trait StatePersistence<S>: Send + Sync {
    /// Error type for state persistence operations.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Load persisted state, if any.
    ///
    /// Returns `None` when no state exists or schema version is stale.
    fn load(
        &self,
        id: &impl Id,
    ) -> impl Future<Output = Result<Option<(S, Version)>, Self::Error>> + Send;

    /// Persist state at the given version.
    fn save(
        &self,
        id: &impl Id,
        version: Version,
        state: &S,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// NoStatePersistence — state persistence disabled
// ═══════════════════════════════════════════════════════════════════════════

/// No-op state persistence. `load` returns `None`, `save` discards.
///
/// Used as the default when `.state_store()` is not called on the builder.
#[derive(Debug, Clone, Copy)]
pub struct NoStatePersistence;

impl<S: Send + Sync> StatePersistence<S> for NoStatePersistence {
    type Error = Infallible;

    async fn load(&self, _id: &impl Id) -> Result<Option<(S, Version)>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _version: Version, _state: &S) -> Result<(), Infallible> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WithStatePersistence — state persistence enabled
// ═══════════════════════════════════════════════════════════════════════════

/// State persistence backed by a [`StateStore`] and [`Codec`].
///
/// Created by the builder's `.state_store(store, codec)` method.
pub struct WithStatePersistence<SS, SC> {
    pub(crate) store: SS,
    pub(crate) codec: SC,
    pub(crate) schema_version: NonZeroU32,
}

impl<SS, SC> WithStatePersistence<SS, SC> {
    /// Create a new state persistence with the given store, codec, and schema version.
    #[must_use]
    pub fn new(store: SS, codec: SC, schema_version: NonZeroU32) -> Self {
        Self {
            store,
            codec,
            schema_version,
        }
    }
}

impl<S, SS, SC> StatePersistence<S> for WithStatePersistence<SS, SC>
where
    S: Send + Sync,
    SS: StateStore,
    SC: Codec<S>,
{
    type Error = StatePersistError<SS::Error, SC::Error>;

    async fn load(&self, id: &impl Id) -> Result<Option<(S, Version)>, Self::Error> {
        let persisted = self
            .store
            .load(id, self.schema_version)
            .await
            .map_err(StatePersistError::Store)?;

        persisted
            .map(|p| {
                let state = self
                    .codec
                    .decode("projection_state", p.payload())
                    .map_err(StatePersistError::Codec)?;
                Ok((state, p.version()))
            })
            .transpose()
    }

    async fn save(&self, id: &impl Id, version: Version, state: &S) -> Result<(), Self::Error> {
        let payload = self.codec.encode(state).map_err(StatePersistError::Codec)?;
        let pending = PendingState::new(version, self.schema_version, payload);
        self.store
            .save(id, &pending)
            .await
            .map_err(StatePersistError::Store)
    }
}
