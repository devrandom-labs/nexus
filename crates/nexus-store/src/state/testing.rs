use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use nexus::Id;
use tokio::sync::RwLock;

use super::pending::PendingState;
use super::persisted::PersistedState;
use super::store::StateStore;

/// In-memory state store for tests.
#[derive(Debug, Default)]
pub struct InMemoryStateStore<S> {
    states: RwLock<HashMap<String, PersistedState<S>>>,
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
    ) -> Result<Option<PersistedState<S>>, Infallible> {
        let states = self.states.read().await;
        let key = id.to_string();
        Ok(states
            .get(&key)
            .filter(|s| s.schema_version() == schema_version)
            .cloned())
    }

    async fn save(&self, id: &impl Id, state: &PendingState<S>) -> Result<(), Infallible> {
        let key = id.to_string();
        let persisted = PersistedState::new(
            state.version(),
            state.schema_version(),
            state.state().clone(),
        );
        self.states.write().await.insert(key, persisted);
        Ok(())
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Infallible> {
        self.states.write().await.remove(&id.to_string());
        Ok(())
    }
}
