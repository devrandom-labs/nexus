#![allow(dead_code)]
use super::repository::EventSourceRepository;
use std::sync::Arc;

#[derive(Default)]
pub struct CommandDispatcherBuilder<R, AppServices>
where
    R: EventSourceRepository + 'static,
    AppServices: Send + Sync + Clone + 'static,
{
    repository: Option<Arc<R>>,
    shared_services: Option<Arc<AppServices>>,
}
