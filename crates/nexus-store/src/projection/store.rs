use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::Id;

use super::pending::PendingState;
use super::persisted::PersistedState;

/// Byte-level projection state storage trait.
///
/// Adapters (fjall, postgres, etc.) implement this to persist and
/// retrieve serialized projection state.
///
/// `()` is the no-op implementation: `load` always returns `None`,
/// `save` and `delete` silently discard. Used when projection state
/// storage is not configured.
pub trait StateStore: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the most recent state for the given projection.
    ///
    /// Returns `None` if no state exists or if the stored state's
    /// schema version does not match `schema_version`. Filtering at the
    /// store level avoids loading payload bytes for stale state.
    fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<PersistedState>, Self::Error>> + Send;

    /// Persist projection state.
    ///
    /// Overwrites any existing state for this projection.
    fn save(
        &self,
        id: &impl Id,
        state: &PendingState,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete projection state.
    ///
    /// Returns `Ok(())` if no state exists (idempotent).
    fn delete(&self, id: &impl Id) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: StateStore> StateStore for &T {
    type Error = T::Error;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Self::Error> {
        (**self).load(id, schema_version).await
    }

    async fn save(&self, id: &impl Id, state: &PendingState) -> Result<(), Self::Error> {
        (**self).save(id, state).await
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        (**self).delete(id).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — projection state storage disabled
// ═══════════════════════════════════════════════════════════════════════════

impl StateStore for () {
    type Error = Infallible;

    async fn load(
        &self,
        _id: &impl Id,
        _schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _state: &PendingState) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}
