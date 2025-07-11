#![allow(dead_code)]
use super::write_side_setup::{User, UserDomainEvents};
use async_trait::async_trait;

use nexus::{
    command::{EventSourceRepository, RepositoryError},
    domain::{Aggregate, AggregateRoot, AggregateType},
    infra::NexusId,
};
use std::collections::{HashMap, hash_map::Entry};
use std::{
    default::Default,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tower::BoxError;

#[derive(Debug, Clone, Copy)]
pub enum ErrorTypes {
    DeserializationError,
    SerializationError,
    StoreError,
}

#[derive(Debug, Error)]
pub enum MockRepositoryError {
    #[error("Mock DB some error")]
    Db,
    #[error("Mock DB Serialization error")]
    Serialization,
    #[error("Mock DB Deserialization error")]
    Deserialization,
}

#[derive(Clone, Debug, Default)]
pub struct MockRepository {
    store: Arc<Mutex<HashMap<NexusId, Vec<UserDomainEvents>>>>,
    error: Option<ErrorTypes>,
}

impl MockRepository {
    pub fn new(initial_data: Option<HashMap<NexusId, Vec<UserDomainEvents>>>) -> Self {
        if let Some(data) = initial_data {
            MockRepository {
                store: Arc::new(Mutex::new(data)),
                error: None,
            }
        } else {
            MockRepository::default()
        }
    }

    pub fn new_with_error(error_type: ErrorTypes) -> Self {
        MockRepository {
            store: Arc::new(Mutex::new(HashMap::new())),
            error: Some(error_type),
        }
    }
}

#[async_trait]
impl EventSourceRepository for MockRepository {
    type AggregateType = User;

    async fn load(
        &self,
        id: &<Self::AggregateType as AggregateType>::Id,
    ) -> Result<
        AggregateRoot<Self::AggregateType>,
        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
    > {
        let id = id.clone();
        let store = Arc::clone(&self.store);
        let error = self.error;
        if let Some(error) = error {
            match error {
                ErrorTypes::DeserializationError => Err(RepositoryError::DeserializationError {
                    source: BoxError::from(MockRepositoryError::Deserialization),
                }),
                ErrorTypes::SerializationError => Err(RepositoryError::SerializationError {
                    source: BoxError::from(MockRepositoryError::Serialization),
                }),
                ErrorTypes::StoreError => Err(RepositoryError::StoreError {
                    source: BoxError::from(MockRepositoryError::Db),
                }),
            }
        } else {
            let store_guard = store.lock().unwrap();
            let aggregate = if let Some(history) = store_guard.get(&id) {
                AggregateRoot::<Self::AggregateType>::load_from_history(id.clone(), history)
            } else {
                return Err(RepositoryError::AggregateNotFound(id));
            };
            Ok(aggregate)
        }
    }

    async fn save(
        &self,
        mut aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Result<(), RepositoryError<<Self::AggregateType as AggregateType>::Id>> {
        let store = Arc::clone(&self.store);
        let error = self.error;
        if let Some(error) = error {
            match error {
                ErrorTypes::DeserializationError => Err(RepositoryError::DeserializationError {
                    source: BoxError::from(MockRepositoryError::Deserialization),
                }),
                ErrorTypes::SerializationError => Err(RepositoryError::SerializationError {
                    source: BoxError::from(MockRepositoryError::Serialization),
                }),
                ErrorTypes::StoreError => Err(RepositoryError::StoreError {
                    source: BoxError::from(MockRepositoryError::Db),
                }),
            }
        } else {
            let version = aggregate.version();
            let events = aggregate.take_uncommitted_events().to_vec();
            let id = aggregate.id();
            let mut store_guard = store.lock().unwrap();
            match store_guard.entry(id.clone()) {
                Entry::Occupied(mut entry) => {
                    let current_events = entry.get_mut();
                    if current_events.len() != version as usize {
                        Err(RepositoryError::Conflict {
                            aggregate_id: id.clone(),
                            expected_version: version,
                        })
                    } else {
                        current_events.extend(events);
                        Ok(())
                    }
                }
                Entry::Vacant(entry) => {
                    if version == 0 {
                        entry.insert(events);
                        Ok(())
                    } else {
                        Err(RepositoryError::Conflict {
                            aggregate_id: id.clone(),
                            expected_version: version,
                        })
                    }
                }
            }
        }
    }
}
