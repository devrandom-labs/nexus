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

impl<R, AppServices> CommandDispatcherBuilder<R, AppServices>
where
    R: EventSourceRepository + 'static,
    AppServices: Send + Sync + Clone + 'static,
{
    pub fn new() -> Self {
        CommandDispatcherBuilder {
            repository: None,
            shared_services: None,
        }
    }

    pub fn repository(mut self, repository: R) -> Self {
        self.repository = Some(Arc::new(repository));
        self
    }

    pub fn shared_services(mut self, shared_services: AppServices) -> Self {
        self.shared_services = Some(Arc::new(shared_services));
        self
    }
}
