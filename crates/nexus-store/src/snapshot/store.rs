use std::convert::Infallible;
use std::future::Future;

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
    /// Returns `None` if no snapshot exists.
    fn load_snapshot(
        &self,
        id: &impl Id,
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
// No-op implementation — snapshots disabled
// ═══════════════════════════════════════════════════════════════════════════

impl SnapshotStore for () {
    type Error = Infallible;

    async fn load_snapshot(&self, _id: &impl Id) -> Result<Option<PersistedSnapshot>, Infallible> {
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
