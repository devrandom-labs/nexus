use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::Id;

use super::pending::PendingSnapshot;
use super::persisted::PersistedSnapshot;

/// Byte-level snapshot storage trait.
///
/// Adapters (fjall, postgres, etc.) implement this to persist and
/// retrieve serialized aggregate state.
///
/// `()` is the no-op implementation: `load_snapshot` always returns
/// `None`, `save_snapshot` and `delete_snapshot` silently discard.
/// Used when snapshot support is not configured.
pub trait SnapshotStore: Send + Sync {
    /// Adapter-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the most recent snapshot for the given aggregate.
    ///
    /// Returns `None` if no snapshot exists or if the stored snapshot's
    /// schema version does not match `schema_version`. Filtering at the
    /// store level avoids loading payload bytes for stale snapshots.
    fn load_snapshot(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> impl Future<Output = Result<Option<PersistedSnapshot>, Self::Error>> + Send;

    /// Persist a snapshot for the given aggregate.
    ///
    /// Overwrites any existing snapshot for this aggregate.
    fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete the snapshot for the given aggregate.
    ///
    /// Returns `Ok(())` if no snapshot exists (idempotent).
    fn delete_snapshot(&self, id: &impl Id)
    -> impl Future<Output = Result<(), Self::Error>> + Send;
}

// ═══════════════════════════════════════════════════════════════════════════
// Delegation implementation — share via reference
// ═══════════════════════════════════════════════════════════════════════════

impl<T: SnapshotStore> SnapshotStore for &T {
    type Error = T::Error;

    async fn load_snapshot(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedSnapshot>, Self::Error> {
        (**self).load_snapshot(id, schema_version).await
    }

    async fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> Result<(), Self::Error> {
        (**self).save_snapshot(id, snapshot).await
    }

    async fn delete_snapshot(&self, id: &impl Id) -> Result<(), Self::Error> {
        (**self).delete_snapshot(id).await
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// No-op implementation — snapshots disabled
// ═══════════════════════════════════════════════════════════════════════════

impl SnapshotStore for () {
    type Error = Infallible;

    async fn load_snapshot(
        &self,
        _id: &impl Id,
        _schema_version: NonZeroU32,
    ) -> Result<Option<PersistedSnapshot>, Infallible> {
        Ok(None)
    }

    async fn save_snapshot(
        &self,
        _id: &impl Id,
        _snapshot: &PendingSnapshot,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn delete_snapshot(&self, _id: &impl Id) -> Result<(), Infallible> {
        Ok(())
    }
}
