#![allow(dead_code)]
use nexus::{
    command::EventSourceRepository,
    domain::{Aggregate, Command},
    error::Error,
};
use std::{marker::PhantomData, sync::Arc};
use terrors::OneOf;

pub mod generic;
pub mod reject;

pub struct AggregateService<A, R, S>
where
    A: Aggregate,
    R: EventSourceRepository<Aggregate = A>,
    S: Send + Sync,
{
    repository: Arc<R>,
    service: Option<Arc<S>>,
    _marker: PhantomData<A>,
}

impl<A, R, S> AggregateService<A, R, S>
where
    A: Aggregate,
    R: EventSourceRepository<Aggregate = A>,
    S: Send + Sync,
{
    pub fn new(repo: R) -> Self {
        AggregateService {
            repository: Arc::new(repo),
            service: None,
            _marker: PhantomData,
        }
    }

    pub fn with_service(&mut self, service: S) {
        self.service.replace(Arc::new(service));
    }

    pub async fn execute<C>(&self, _command: C) -> Result<C::Result, OneOf<(Error, C::Error)>>
    where
        C: Command,
    {
        // TODO: extract stream_id from command
        // TODO: load aggregate with that stream_id
        // TODO: fetch the aggregate command handler from the inmemory command handler store.
        // TODO: call aggregate execute
        // TODO: return result or error
        //
        unimplemented!()
    }
}
