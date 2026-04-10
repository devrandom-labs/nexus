use std::collections::HashMap;
use std::convert::Infallible;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingSnapshot;
use super::persisted::PersistedSnapshot;
use super::store::SnapshotStore;

/// In-memory snapshot store for tests.
///
/// Stores snapshots in a `HashMap` protected by a `RwLock`.
#[derive(Debug, Default)]
pub struct InMemorySnapshotStore {
    snapshots: RwLock<HashMap<String, StoredSnapshot>>,
}

#[derive(Debug, Clone)]
struct StoredSnapshot {
    version: nexus::Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl InMemorySnapshotStore {
    /// Create a new empty in-memory snapshot store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SnapshotStore for InMemorySnapshotStore {
    type Error = Infallible;

    async fn load_snapshot(&self, id: &impl Id) -> Result<Option<PersistedSnapshot>, Infallible> {
        let snapshots = self.snapshots.read().await;
        let key = id.to_string();
        Ok(snapshots
            .get(&key)
            .map(|s| PersistedSnapshot::new(s.version, s.schema_version, s.payload.clone())))
    }

    async fn save_snapshot(
        &self,
        id: &impl Id,
        snapshot: &PendingSnapshot,
    ) -> Result<(), Infallible> {
        let mut snapshots = self.snapshots.write().await;
        let key = id.to_string();
        snapshots.insert(
            key,
            StoredSnapshot {
                version: snapshot.version(),
                schema_version: snapshot.schema_version(),
                payload: snapshot.payload().to_vec(),
            },
        );
        Ok(())
    }

    async fn delete_snapshot(&self, id: &impl Id) -> Result<(), Infallible> {
        let mut snapshots = self.snapshots.write().await;
        snapshots.remove(&id.to_string());
        Ok(())
    }
}
