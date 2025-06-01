#![allow(dead_code)]
use super::{User, UserDomainEvents};
use crate::command::repository::EventSourceRepository;
use crate::command::{
    aggregate::{Aggregate, AggregateRoot, AggregateType},
    repository::RepositoryError,
};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
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
        _id: &'a <Self::AggregateType as AggregateType>::Id,
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
        todo!("get the vector")
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
        Box::pin(async move {
            let mut store = self.store.lock().unwrap();
            let version = aggregate.version();
            let events = aggregate.take_uncommitted_events().to_vec();

            match store.entry(aggregate.id().into()) {
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
