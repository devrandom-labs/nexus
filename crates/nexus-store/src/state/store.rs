use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::Id;

use super::pending::PendingState;
use super::persisted::PersistedState;

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
    ) -> impl Future<Output = Result<Option<PersistedState<S>>, Self::Error>> + Send;

    /// Persist state (overwrites existing).
    fn save(
        &self,
        id: &impl Id,
        state: &PendingState<S>,
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
    ) -> Result<Option<PersistedState<S>>, Self::Error> {
        (**self).load(id, schema_version).await
    }

    async fn save(&self, id: &impl Id, state: &PendingState<S>) -> Result<(), Self::Error> {
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
    ) -> Result<Option<PersistedState<S>>, Infallible> {
        Ok(None)
    }

    async fn save(&self, _id: &impl Id, _state: &PendingState<S>) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}
