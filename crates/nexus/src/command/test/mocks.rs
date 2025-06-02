#![allow(dead_code)]
use super::{
    User, UserDomainEvents,
    utils::{EventType, MockData},
};
use crate::command::{
    aggregate::{Aggregate, AggregateRoot, AggregateType},
    repository::{EventSourceRepository, RepositoryError},
};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, hash_map::Entry};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default)]
pub struct MockRepository {
    store: Arc<Mutex<HashMap<String, Vec<UserDomainEvents>>>>,
}

impl MockRepository {
    pub fn new(timestamp: Option<DateTime<Utc>>, event_type: EventType) -> Self {
        match event_type {
            EventType::Empty => MockRepository::default(),
            _ => MockRepository {
                store: Arc::new(Mutex::new(
                    MockData::new(event_type, timestamp).get_hash_map(),
                )),
            },
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
        Box::pin(async move {
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
        Box::pin(async move {
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
        })
    }
}
