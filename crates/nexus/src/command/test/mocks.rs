#![allow(dead_code)]
use super::{User, UserDomainEvents};
use crate::command::{
    aggregate::{Aggregate, AggregateRoot, AggregateType},
    repository::{EventSourceRepository, RepositoryError},
};
use std::collections::{HashMap, hash_map::Entry};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct MockRepository {
    store: Arc<Mutex<HashMap<String, Vec<UserDomainEvents>>>>,
}

impl EventSourceRepository for MockRepository {
    type AggregateType = User;

    fn load<'a>(
        &'a self,
        id: &'a <Self::AggregateType as AggregateType>::Id,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        AggregateRoot<Self::AggregateType>,
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    > {
        let id = id.clone();
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let store_gaurd = store.lock().unwrap();
            let aggregate = if let Some(history) = store_gaurd.get(&id) {
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

    fn save<'a>(
        &'a self,
        mut aggregate: AggregateRoot<Self::AggregateType>,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (),
                        RepositoryError<<Self::AggregateType as AggregateType>::Id>,
                    >,
                > + Send
                + 'a,
        >,
    > {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let version = aggregate.version();
            let events = aggregate.take_uncommitted_events().to_vec();
            let mut store_guard = store.lock().unwrap();
            match store_guard.entry(aggregate.id().into()) {
                Entry::Occupied(mut entry) => {
                    let current_events = entry.get_mut();
                    if current_events.len() != version as usize {
                        Err(RepositoryError::Conflict {
                            aggregate_id: aggregate.id().to_string(),
                            expected_version: aggregate.version(),
                        })
                    } else {
                        current_events.extend(events);
                        Ok(())
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(events);
                    Ok(())
                }
            }
        })
    }
}
