#![allow(dead_code)]
use crate::{
    Command,
    aggregate::{AggregateRoot, AggregateType, command_handler::AggregateCommandHandler},
};
use std::{fmt::Debug, marker::PhantomData, pin::Pin, task::Context, task::Poll};
use tower::Service;

#[derive(Debug)]
pub struct InvokeRequest<AT, C>
where
    AT: AggregateType,
    C: Command,
{
    pub aggregate: AggregateRoot<AT>,
    pub command: C,
}

#[derive(Debug)]
pub struct InvokeResponse<AT, R>
where
    AT: AggregateType,
    R: Send + Sync + Debug + 'static,
{
    pub aggregate: AggregateRoot<AT>,
    pub result: R,
}

#[derive(Debug, Clone)]
pub struct InvokeHandlerService<AT, Services, H, C>
where
    AT: AggregateType,
    Services: Send + Sync + Clone + 'static,
    C: Command,
    H: AggregateCommandHandler<C, Services, AggregateType = AT> + Clone + 'static,
{
    services: Services,
    handler: H,
    _command: PhantomData<C>,
    _aggregate_type: PhantomData<AT>,
}

impl<AT, Services, H, C> Service<InvokeRequest<AT, C>> for InvokeHandlerService<AT, Services, H, C>
where
    AT: AggregateType,
    Services: Send + Sync + Clone,
    C: Command,
    H: AggregateCommandHandler<C, Services, AggregateType = AT> + Clone,
{
    type Error = C::Error;
    type Response = InvokeResponse<AT, C::Result>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: InvokeRequest<AT, C>) -> Self::Future {
        let handler = self.handler.clone();
        let services = self.services.clone();
        Box::pin(async move {
            let mut aggregate = req.aggregate;
            let result = aggregate.execute(req.command, &handler, &services).await?;
            Ok(InvokeResponse { aggregate, result })
        })
    }
}
