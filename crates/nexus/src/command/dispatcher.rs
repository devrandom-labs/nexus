#![allow(dead_code)]
use super::{
    aggregate::AggregateType, handler::AggregateCommandHandler, repository::EventSourceRepository,
};
use crate::{BoxMessage, Command, ErasedFuture, error::RegistrationFailed};
use std::{
    any::{Any, TypeId},
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

pub trait DispatcherPipeline: Send + Sync {
    fn run(&mut self, command: BoxMessage) -> ErasedFuture;
}

type PipelineFactory<R, AppServices> = Box<
    dyn FnOnce(Arc<R>, Arc<AppServices>) -> Box<dyn DispatcherPipeline + Send + Sync>
        + Send
        + Sync
        + 'static,
>;

#[derive(Default)]
pub struct CommandDispatcherBuilder<R, AppServices>
where
    R: EventSourceRepository + 'static,
    AppServices: Send + Sync + Clone + 'static,
{
    repository: Option<Arc<R>>,
    shared_services: Option<Arc<AppServices>>,
    factories: HashMap<TypeId, PipelineFactory<R, AppServices>>,
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
            factories: HashMap::new(),
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

    pub fn register<H, C, AT, S>(
        mut self,
        _handler: H,
        _services: S,
    ) -> Result<Self, RegistrationFailed>
    where
        H: AggregateCommandHandler<C, S, AggregateType = AT> + 'static + Clone,
        C: Command + Any,
        S: Send + Sync + Clone + 'static,
        AT: AggregateType,
    {
        let command_id = TypeId::of::<C>();
        let command_name = std::any::type_name::<C>();

        match self.factories.entry(command_id) {
            Entry::Occupied(_) => Err(RegistrationFailed(format!(
                "Pipeline exists for {} command",
                command_name
            ))),
            Entry::Vacant(_entry) => {
                // let factory = Box::new(move |r: Arc<R>, s: Arc<AppServices>| {
                //     let load_service = LoadService::new(r.clone());
                //     Box::pin(async move {
                //         // TODO: impl DispatchHandler
                //         unimplemented!();
                //     })
                // });
                // entry.insert(factory);
                Ok(())
            }
        }?;

        Ok(self)
    }
}
