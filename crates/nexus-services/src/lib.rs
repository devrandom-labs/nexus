use nexus::{command::EventSourceRepository, domain::Aggregate};
use std::{any::TypeId, collections::HashMap, marker::PhantomData, sync::Arc};

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
}
