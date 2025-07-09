#![allow(dead_code)]
use super::{
    utils::{EventType, MockData},
    write_side_setup::{User, UserDomainEvents},
};
use chrono::{DateTime, Utc};
use nexus::{
    command::{EventSourceRepository, RepositoryError},
    domain::{Aggregate, AggregateRoot, AggregateType},
};
use std::collections::{HashMap, hash_map::Entry};
use std::{
    default::Default,
    pin::Pin,
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
    store: Arc<Mutex<HashMap<String, Vec<UserDomainEvents>>>>,
    error: Option<ErrorTypes>,
}

impl MockRepository {
    pub fn new(timestamp: Option<DateTime<Utc>>, event_type: EventType) -> Self {
        match event_type {
            EventType::Empty => MockRepository::default(),
            _ => MockRepository {
                store: Arc::new(Mutex::new(
                    MockData::new(timestamp, event_type).get_hash_map(),
                )),
                error: None,
            },
        }
    }

    pub fn new_with_error(error_type: ErrorTypes) -> Self {
        MockRepository {
            store: Arc::new(Mutex::new(HashMap::new())),
            error: Some(error_type),
        }
    }
}

impl EventSourceRepository for MockRepository {
    type AggregateType = User;

    fn load(
        &self,
        id: &<Self::AggregateType as AggregateType>::Id,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'static,
        >,
    > {
        let id = id.clone();
        let store = Arc::clone(&self.store);
        let error = self.error;
        Box::pin(async move {
            if let Some(error) = error {
                match error {
                    ErrorTypes::DeserializationError => {
                        Err(RepositoryError::DeserializationError {
                            source: BoxError::from(MockRepositoryError::Deserialization),
                        })
                    }
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
                        .map_err(|err| RepositoryError::DataIntegrityError {
                            aggregate_id: id,
                            source: err,
                        })?
                } else {
                    return Err(RepositoryError::AggregateNotFound(id));
                };
                Ok(aggregate)
            }
        })
    }

    fn save(
        &self,
        mut aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (),
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'static,
        >,
    > {
        let store = Arc::clone(&self.store);
        let error = self.error;
        Box::pin(async move {
            if let Some(error) = error {
                match error {
                    ErrorTypes::DeserializationError => {
                        Err(RepositoryError::DeserializationError {
                            source: BoxError::from(MockRepositoryError::Deserialization),
                        })
                    }
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
                let id = aggregate.id().to_string();
                let mut store_guard = store.lock().unwrap();
                match store_guard.entry(id.clone()) {
                    Entry::Occupied(mut entry) => {
                        let current_events = entry.get_mut();
                        if current_events.len() != version as usize {
                            Err(RepositoryError::Conflict {
                                aggregate_id: id,
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
                                aggregate_id: id,
                                expected_version: version,
                            })
                        }
                    }
                }
            }
        })
    }
}
