use super::model::ReadModel;
use std::{boxed::Box, error::Error as StdError, future::Future, pin::Pin};

pub trait ReadModelRepository: Send + Sync + Clone {
    type Error: Send + Sync + Clone + StdError;
    type Model: ReadModel;

    #[allow(clippy::type_complexity)]
    fn get<'a>(
        &'a self,
        id: &'a <Self::Model as ReadModel>::Id,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Model, Self::Error>> + Send + 'a>>;
}
