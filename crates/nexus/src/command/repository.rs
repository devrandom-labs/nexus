use crate::domain::Aggregate;
use crate::error::Result;
use async_trait::async_trait;
use std::fmt::Debug;

#[async_trait]
pub trait EventSourceRepository: Send + Sync + Clone + Debug {
    type Aggregate: Aggregate;
    async fn load(&self, id: &<Self::Aggregate as Aggregate>::Id) -> Result<Self::Aggregate>;
    async fn save(&self, aggregate: &mut Self::Aggregate) -> Result<()>;
}
