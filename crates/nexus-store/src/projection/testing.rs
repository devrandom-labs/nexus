use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingState;
use super::persisted::PersistedState;
use super::store::StateStore;

/// In-memory projection state store for tests.
///
/// Stores projection state in a `HashMap` protected by a `RwLock`.
#[derive(Debug, Default)]
pub struct InMemoryStateStore {
    states: RwLock<HashMap<String, StoredState>>,
}

#[derive(Debug, Clone)]
struct StoredState {
    version: nexus::Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl InMemoryStateStore {
    /// Create a new empty in-memory state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl StateStore for InMemoryStateStore {
    type Error = Infallible;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState>, Infallible> {
        let states = self.states.read().await;
        let key = id.to_string();
        Ok(states
            .get(&key)
            .filter(|s| s.schema_version == schema_version)
            .map(|s| PersistedState::new(s.version, s.schema_version, s.payload.clone())))
    }

    async fn save(&self, id: &impl Id, state: &PendingState) -> Result<(), Infallible> {
        let key = id.to_string();
        self.states.write().await.insert(
            key,
            StoredState {
                version: state.version(),
                schema_version: state.schema_version(),
                payload: state.payload().to_vec(),
            },
        );
        Ok(())
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Infallible> {
        self.states.write().await.remove(&id.to_string());
        Ok(())
    }
}
