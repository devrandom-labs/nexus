#![allow(dead_code)]
use super::{User, UserDomainEvents};
use crate::command::repository::EventSourceRepository;
use crate::command::{
    aggregate::{Aggregate, AggregateRoot, AggregateType},
    repository::RepositoryError,
};
use std::collections::HashMap;
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
        aggregate: AggregateRoot<Self::AggregateType>,
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
            let current_events = store
                .get_mut(aggregate.id())
                .ok_or(RepositoryError::AggregateNotFound(aggregate.id().clone()))?;

            if current_events.len() as u64 != aggregate.version() {
                Err(RepositoryError::Conflict {
                    aggregate_id: aggregate.id().to_string(),
                    expected_version: aggregate.version(),
                })
            } else {
                // TODO: expand the current events and insert it back, we can use hashmaps entries way of doing things
                Ok(())
            }
        })
    }
}
